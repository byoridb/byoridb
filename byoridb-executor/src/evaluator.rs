// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Expression evaluator for WHERE clauses
//!
//! Supports:
//! - Comparison operators: ==, !=, <, <=, >, >=
//! - Logical operators: AND, OR, NOT
//! - Arithmetic operators: +, -, *, /, %
//! - Property access: tag.prop, $$.tag.prop, $^.tag.prop

use crate::error::{ExecutionError, Result};
use byoridb_common::datatypes::list::List;
use byoridb_common::datatypes::map::Map;
use byoridb_common::Value;
use byoridb_parser::ast::{BinaryOperator, Expression, Literal, UnaryOperator};
use std::collections::HashMap;

/// Every scalar function name [`Evaluator::apply_scalar_function`] accepts.
///
/// It exists because a caller sometimes has to know whether a name is supported
/// *before* evaluating it: the MATCH planner rejects an unknown function up
/// front rather than letting it evaluate to `NULL` and look like empty data
/// (#102). Without this list that caller would keep a second copy of the
/// dispatcher's names, and the two would drift — the test
/// `every_listed_scalar_function_is_dispatched` is what keeps them honest.
pub const SCALAR_FUNCTIONS: &[&str] = &[
    "LOWER",
    "TOLOWER",
    "UPPER",
    "TOUPPER",
    "LENGTH",
    "SIZE",
    "CONTAINS",
    "STARTS_WITH",
    "STARTSWITH",
    "ENDS_WITH",
    "ENDSWITH",
    "ABS",
    "FLOOR",
    "CEIL",
    "ROUND",
    "IS_NULL",
    "ISNULL",
    "IS_NOT_NULL",
    "ISNOTNULL",
    "COALESCE",
];

/// Evaluation context containing variable bindings
#[derive(Debug, Clone, Default)]
pub struct EvalContext {
    /// Current vertex properties (for MATCH/GO queries)
    pub current: HashMap<String, Value>,
    /// Source vertex properties ($^)
    pub source: HashMap<String, Value>,
    /// Destination vertex properties ($$)
    pub destination: HashMap<String, Value>,
    /// Named variables (e.g., n, m in MATCH (n)-[e]->(m))
    pub variables: HashMap<String, HashMap<String, Value>>,
}

impl EvalContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set current vertex properties
    pub fn with_current(mut self, props: HashMap<String, Value>) -> Self {
        self.current = props;
        self
    }

    /// Set source vertex properties
    pub fn with_source(mut self, props: HashMap<String, Value>) -> Self {
        self.source = props;
        self
    }

    /// Set destination vertex properties
    pub fn with_destination(mut self, props: HashMap<String, Value>) -> Self {
        self.destination = props;
        self
    }

    /// Add a named variable
    pub fn with_variable(mut self, name: &str, props: HashMap<String, Value>) -> Self {
        self.variables.insert(name.to_string(), props);
        self
    }

    /// Get a property value by path (e.g., "person.age", "$$.person.name")
    pub fn get_property(&self, path: &str) -> Option<Value> {
        let parts: Vec<&str> = path.split('.').collect();

        match parts.as_slice() {
            // $$.tag.prop - destination vertex
            ["$$", tag, prop] => self
                .destination
                .get(&format!("{}.{}", tag, prop))
                .or_else(|| self.destination.get(*prop))
                .cloned(),
            // $^.tag.prop - source vertex
            ["$^", tag, prop] => self
                .source
                .get(&format!("{}.{}", tag, prop))
                .or_else(|| self.source.get(*prop))
                .cloned(),
            // tag.prop - current context or variable
            [tag, prop] => {
                // First check if it's a named variable
                if let Some(var_props) = self.variables.get(*tag) {
                    return var_props.get(*prop).cloned();
                }
                // Then check current context
                self.current
                    .get(&format!("{}.{}", tag, prop))
                    .or_else(|| self.current.get(*prop))
                    .cloned()
            }
            // Single identifier - check current and variables
            [name] => {
                if let Some(v) = self.current.get(*name) {
                    return Some(v.clone());
                }
                // Check all variables
                for var_props in self.variables.values() {
                    if let Some(v) = var_props.get(*name) {
                        return Some(v.clone());
                    }
                }
                None
            }
            _ => None,
        }
    }
}

/// Expression evaluator
pub struct Evaluator;

impl Evaluator {
    /// Evaluate an expression without context (for literals only)
    pub fn evaluate(expr: &Expression) -> Result<Value> {
        Self::evaluate_with_context(expr, &EvalContext::new())
    }

    /// Evaluate an expression with context
    pub fn evaluate_with_context(expr: &Expression, ctx: &EvalContext) -> Result<Value> {
        match expr {
            Expression::Literal(lit) => Ok(Self::eval_literal(lit)),

            Expression::Identifier(name) => {
                // Try to resolve from context
                ctx.get_property(name).ok_or_else(|| {
                    ExecutionError::TypeMismatch(format!("Unknown identifier: {}", name))
                })
            }

            Expression::BinaryOp { op, left, right } => {
                let l = Self::evaluate_with_context(left, ctx)?;
                let r = Self::evaluate_with_context(right, ctx)?;
                Self::eval_binary_op(op, l, r)
            }

            Expression::UnaryOp { op, operand } => {
                let v = Self::evaluate_with_context(operand, ctx)?;
                Self::eval_unary_op(op, v)
            }

            Expression::FunctionCall { name, args } => Self::eval_function(name, args, ctx),

            Expression::List(items) => {
                let values: Result<Vec<Value>> = items
                    .iter()
                    .map(|e| Self::evaluate_with_context(e, ctx))
                    .collect();
                Ok(Value::List(List { values: values? }))
            }

            Expression::Map(map) => {
                let mut result = HashMap::new();
                for (k, v) in map {
                    result.insert(k.clone(), Self::evaluate_with_context(v, ctx)?);
                }
                Ok(Value::Map(Map { data: result }))
            }

            // PropRef / DstVertexProp require async context (kvstore access).
            // The synchronous evaluator resolves PropRef against the flat
            // property context and treats DstVertexProp as unknown.
            Expression::PropRef { object, prop } => {
                let key = format!("{}.{}", object, prop);
                ctx.get_property(&key)
                    .or_else(|| ctx.get_property(prop))
                    .ok_or_else(|| {
                        ExecutionError::TypeMismatch(format!(
                            "Unknown property reference: {}.{}",
                            object, prop
                        ))
                    })
            }

            Expression::DstVertexProp { tag, prop } => Err(ExecutionError::InvalidOperation(
                format!("$$.{}.{} requires async context; use GO YIELD", tag, prop),
            )),
        }
    }

    /// Evaluate a WHERE clause expression and return boolean result
    pub fn evaluate_condition(expr: &Expression, ctx: &EvalContext) -> Result<bool> {
        let result = Self::evaluate_with_context(expr, ctx)?;
        match result {
            Value::Bool(b) => Ok(b),
            Value::Null(_) => Ok(false),
            _ => Err(ExecutionError::TypeMismatch(format!(
                "WHERE clause must evaluate to boolean, got {:?}",
                result
            ))),
        }
    }

    fn eval_literal(lit: &Literal) -> Value {
        match lit {
            Literal::Bool(b) => Value::Bool(*b),
            Literal::Int(i) => Value::Int(*i),
            Literal::Float(f) => Value::Float(*f),
            Literal::String(s) => Value::String(s.clone()),
            Literal::Null => Value::null(),
        }
    }

    fn eval_binary_op(op: &BinaryOperator, left: Value, right: Value) -> Result<Value> {
        match op {
            // Arithmetic operators
            BinaryOperator::Add => Self::eval_add(left, right),
            BinaryOperator::Sub => Self::eval_sub(left, right),
            BinaryOperator::Mul => Self::eval_mul(left, right),
            BinaryOperator::Div => Self::eval_div(left, right),
            BinaryOperator::Mod => Self::eval_mod(left, right),

            // Comparison operators
            BinaryOperator::Eq => Ok(Value::Bool(Self::values_equal(&left, &right))),
            BinaryOperator::Neq => Ok(Value::Bool(!Self::values_equal(&left, &right))),
            BinaryOperator::Lt => {
                Self::eval_compare(left, right, |ord| ord == std::cmp::Ordering::Less)
            }
            BinaryOperator::Lte => {
                Self::eval_compare(left, right, |ord| ord != std::cmp::Ordering::Greater)
            }
            BinaryOperator::Gt => {
                Self::eval_compare(left, right, |ord| ord == std::cmp::Ordering::Greater)
            }
            BinaryOperator::Gte => {
                Self::eval_compare(left, right, |ord| ord != std::cmp::Ordering::Less)
            }

            // Logical operators
            BinaryOperator::And => Self::eval_and(left, right),
            BinaryOperator::Or => Self::eval_or(left, right),

            // String operators
            BinaryOperator::Contains => match (&left, &right) {
                (Value::String(s), Value::String(sub)) => Ok(Value::Bool(s.contains(sub.as_str()))),
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::null()),
                _ => Ok(Value::Bool(false)),
            },
            BinaryOperator::NotContains => match (&left, &right) {
                (Value::String(s), Value::String(sub)) => {
                    Ok(Value::Bool(!s.contains(sub.as_str())))
                }
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::null()),
                _ => Ok(Value::Bool(true)),
            },

            // Set membership
            BinaryOperator::In => Ok(Self::eval_in(&left, &right)),
            BinaryOperator::NotIn => Ok(match Self::eval_in(&left, &right) {
                Value::Bool(found) => Value::Bool(!found),
                other => other,
            }),
            BinaryOperator::StartsWith => match (&left, &right) {
                (Value::String(s), Value::String(pre)) => {
                    Ok(Value::Bool(s.starts_with(pre.as_str())))
                }
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::null()),
                _ => Ok(Value::Bool(false)),
            },
            BinaryOperator::EndsWith => match (&left, &right) {
                (Value::String(s), Value::String(suf)) => {
                    Ok(Value::Bool(s.ends_with(suf.as_str())))
                }
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::null()),
                _ => Ok(Value::Bool(false)),
            },

            // Regex match: string =~ "pattern"
            BinaryOperator::Regex => match (&left, &right) {
                (Value::String(s), Value::String(pat)) => Ok(Value::Bool(
                    regex::Regex::new(pat)
                        .map(|re| re.is_match(s))
                        .unwrap_or(false),
                )),
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::null()),
                _ => Ok(Value::Bool(false)),
            },
        }
    }

    fn eval_unary_op(op: &UnaryOperator, operand: Value) -> Result<Value> {
        match op {
            UnaryOperator::Not => match operand {
                Value::Bool(b) => Ok(Value::Bool(!b)),
                Value::Null(_) => Ok(Value::null()),
                _ => Err(ExecutionError::TypeMismatch(format!(
                    "NOT operator requires boolean, got {:?}",
                    operand
                ))),
            },
            UnaryOperator::Neg => match operand {
                Value::Int(i) => Ok(Value::Int(-i)),
                Value::Float(f) => Ok(Value::Float(-f)),
                Value::Null(_) => Ok(Value::null()),
                _ => Err(ExecutionError::TypeMismatch(format!(
                    "Negation requires number, got {:?}",
                    operand
                ))),
            },
        }
    }

    fn eval_function(name: &str, args: &[Expression], ctx: &EvalContext) -> Result<Value> {
        let evaluated_args: Result<Vec<Value>> = args
            .iter()
            .map(|e| Self::evaluate_with_context(e, ctx))
            .collect();
        let args = evaluated_args?;

        match name.to_uppercase().as_str() {
            // Ontology class membership (PLAN.md R-3b): true iff the current
            // vertex's class set (`__isa__`, its tags ∪ transitive superclasses,
            // injected by the caller) contains the named class. Lets a RECOMMEND
            // WHERE express subclass-aware filters like `is_a("animal")`.
            //
            // This one stays here rather than moving to the shared scalar
            // library below, because it reads from the evaluation context and
            // not only from its arguments.
            "IS_A" | "ISA" => {
                let target = match args.first() {
                    Some(Value::String(s)) => s,
                    Some(Value::Null(_)) => return Ok(Value::null()),
                    _ => {
                        return Err(ExecutionError::InvalidOperation(
                            "is_a requires a class-name string argument".to_string(),
                        ))
                    }
                };
                let member = matches!(ctx.get_property("__isa__"), Some(Value::List(l))
                    if l.values.iter().any(|v| matches!(v, Value::String(s) if s == target)));
                Ok(Value::Bool(member))
            }
            _ => Self::apply_scalar_function(name, &args),
        }
    }

    /// Apply a function that depends only on its argument *values*.
    ///
    /// Separated from [`Self::eval_function`] so the MATCH executor, which
    /// resolves arguments against graph bindings rather than an `EvalContext`,
    /// can use these implementations instead of maintaining its own. Before
    /// this existed, MATCH silently returned `NULL` for every function it did
    /// not recognise, including `toLower` (#102).
    pub fn apply_scalar_function(name: &str, args: &[Value]) -> Result<Value> {
        match name.to_uppercase().as_str() {
            // String functions
            "LOWER" | "TOLOWER" => match args.first() {
                Some(Value::String(s)) => Ok(Value::String(s.to_lowercase())),
                Some(Value::Null(_)) => Ok(Value::null()),
                _ => Err(ExecutionError::InvalidOperation(
                    "LOWER requires string argument".to_string(),
                )),
            },
            "UPPER" | "TOUPPER" => match args.first() {
                Some(Value::String(s)) => Ok(Value::String(s.to_uppercase())),
                Some(Value::Null(_)) => Ok(Value::null()),
                _ => Err(ExecutionError::InvalidOperation(
                    "UPPER requires string argument".to_string(),
                )),
            },
            "LENGTH" | "SIZE" => match args.first() {
                Some(Value::String(s)) => Ok(Value::Int(s.len() as i64)),
                Some(Value::List(l)) => Ok(Value::Int(l.len() as i64)),
                Some(Value::Null(_)) => Ok(Value::null()),
                _ => Err(ExecutionError::InvalidOperation(
                    "LENGTH requires string or list".to_string(),
                )),
            },
            "CONTAINS" => match (args.first(), args.get(1)) {
                (Some(Value::String(s)), Some(Value::String(sub))) => {
                    Ok(Value::Bool(s.contains(sub.as_str())))
                }
                (Some(Value::Null(_)), _) | (_, Some(Value::Null(_))) => Ok(Value::null()),
                _ => Err(ExecutionError::InvalidOperation(
                    "CONTAINS requires two string arguments".to_string(),
                )),
            },
            "STARTS_WITH" | "STARTSWITH" => match (args.first(), args.get(1)) {
                (Some(Value::String(s)), Some(Value::String(prefix))) => {
                    Ok(Value::Bool(s.starts_with(prefix.as_str())))
                }
                (Some(Value::Null(_)), _) | (_, Some(Value::Null(_))) => Ok(Value::null()),
                _ => Err(ExecutionError::InvalidOperation(
                    "STARTS_WITH requires two string arguments".to_string(),
                )),
            },
            "ENDS_WITH" | "ENDSWITH" => match (args.first(), args.get(1)) {
                (Some(Value::String(s)), Some(Value::String(suffix))) => {
                    Ok(Value::Bool(s.ends_with(suffix.as_str())))
                }
                (Some(Value::Null(_)), _) | (_, Some(Value::Null(_))) => Ok(Value::null()),
                _ => Err(ExecutionError::InvalidOperation(
                    "ENDS_WITH requires two string arguments".to_string(),
                )),
            },

            // Numeric functions
            "ABS" => match args.first() {
                Some(Value::Int(i)) => Ok(Value::Int(i.abs())),
                Some(Value::Float(f)) => Ok(Value::Float(f.abs())),
                Some(Value::Null(_)) => Ok(Value::null()),
                _ => Err(ExecutionError::InvalidOperation(
                    "ABS requires numeric argument".to_string(),
                )),
            },
            "FLOOR" => match args.first() {
                Some(Value::Float(f)) => Ok(Value::Int(f.floor() as i64)),
                Some(Value::Int(i)) => Ok(Value::Int(*i)),
                Some(Value::Null(_)) => Ok(Value::null()),
                _ => Err(ExecutionError::InvalidOperation(
                    "FLOOR requires numeric argument".to_string(),
                )),
            },
            "CEIL" => match args.first() {
                Some(Value::Float(f)) => Ok(Value::Int(f.ceil() as i64)),
                Some(Value::Int(i)) => Ok(Value::Int(*i)),
                Some(Value::Null(_)) => Ok(Value::null()),
                _ => Err(ExecutionError::InvalidOperation(
                    "CEIL requires numeric argument".to_string(),
                )),
            },
            "ROUND" => match args.first() {
                Some(Value::Float(f)) => Ok(Value::Int(f.round() as i64)),
                Some(Value::Int(i)) => Ok(Value::Int(*i)),
                Some(Value::Null(_)) => Ok(Value::null()),
                _ => Err(ExecutionError::InvalidOperation(
                    "ROUND requires numeric argument".to_string(),
                )),
            },

            // Type checking
            "IS_NULL" | "ISNULL" => match args.first() {
                Some(Value::Null(_)) => Ok(Value::Bool(true)),
                Some(_) => Ok(Value::Bool(false)),
                None => Err(ExecutionError::InvalidOperation(
                    "IS_NULL requires one argument".to_string(),
                )),
            },
            "IS_NOT_NULL" | "ISNOTNULL" => match args.first() {
                Some(Value::Null(_)) => Ok(Value::Bool(false)),
                Some(_) => Ok(Value::Bool(true)),
                None => Err(ExecutionError::InvalidOperation(
                    "IS_NOT_NULL requires one argument".to_string(),
                )),
            },

            // Coalesce
            "COALESCE" => {
                for arg in args {
                    if !matches!(arg, Value::Null(_)) {
                        return Ok(arg.clone());
                    }
                }
                Ok(Value::null())
            }

            _ => Err(ExecutionError::InvalidOperation(format!(
                "Unknown function: {}",
                name
            ))),
        }
    }

    // Arithmetic helpers
    fn eval_add(left: Value, right: Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(l), Value::Int(r)) => Ok(Value::Int(l + r)),
            (Value::Float(l), Value::Float(r)) => Ok(Value::Float(l + r)),
            (Value::Int(l), Value::Float(r)) => Ok(Value::Float(l as f64 + r)),
            (Value::Float(l), Value::Int(r)) => Ok(Value::Float(l + r as f64)),
            (Value::String(l), Value::String(r)) => Ok(Value::String(l + &r)),
            (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::null()),
            (l, r) => Err(ExecutionError::TypeMismatch(format!(
                "Cannot add {:?} and {:?}",
                l, r
            ))),
        }
    }

    fn eval_sub(left: Value, right: Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(l), Value::Int(r)) => Ok(Value::Int(l - r)),
            (Value::Float(l), Value::Float(r)) => Ok(Value::Float(l - r)),
            (Value::Int(l), Value::Float(r)) => Ok(Value::Float(l as f64 - r)),
            (Value::Float(l), Value::Int(r)) => Ok(Value::Float(l - r as f64)),
            (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::null()),
            (l, r) => Err(ExecutionError::TypeMismatch(format!(
                "Cannot subtract {:?} from {:?}",
                r, l
            ))),
        }
    }

    fn eval_mul(left: Value, right: Value) -> Result<Value> {
        match (left, right) {
            (Value::Int(l), Value::Int(r)) => Ok(Value::Int(l * r)),
            (Value::Float(l), Value::Float(r)) => Ok(Value::Float(l * r)),
            (Value::Int(l), Value::Float(r)) => Ok(Value::Float(l as f64 * r)),
            (Value::Float(l), Value::Int(r)) => Ok(Value::Float(l * r as f64)),
            (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::null()),
            (l, r) => Err(ExecutionError::TypeMismatch(format!(
                "Cannot multiply {:?} and {:?}",
                l, r
            ))),
        }
    }

    fn eval_div(left: Value, right: Value) -> Result<Value> {
        match (left, right) {
            (_, Value::Int(0)) => Err(ExecutionError::InvalidOperation(
                "Division by zero".to_string(),
            )),
            (_, Value::Float(0.0)) => Err(ExecutionError::InvalidOperation(
                "Division by zero".to_string(),
            )),
            (Value::Int(l), Value::Int(r)) => Ok(Value::Int(l / r)),
            (Value::Float(l), Value::Float(r)) => Ok(Value::Float(l / r)),
            (Value::Int(l), Value::Float(r)) => Ok(Value::Float(l as f64 / r)),
            (Value::Float(l), Value::Int(r)) => Ok(Value::Float(l / r as f64)),
            (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::null()),
            (l, r) => Err(ExecutionError::TypeMismatch(format!(
                "Cannot divide {:?} by {:?}",
                l, r
            ))),
        }
    }

    fn eval_mod(left: Value, right: Value) -> Result<Value> {
        match (left, right) {
            (_, Value::Int(0)) => Err(ExecutionError::InvalidOperation(
                "Modulo by zero".to_string(),
            )),
            (Value::Int(l), Value::Int(r)) => Ok(Value::Int(l % r)),
            (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::null()),
            (l, r) => Err(ExecutionError::TypeMismatch(format!(
                "Cannot modulo {:?} by {:?}",
                l, r
            ))),
        }
    }

    // Comparison helpers
    /// `left IN right` under three-valued logic: a match wins over an unknown,
    /// and an unknown wins over "not found". So `2 IN [1, NULL]` is NULL rather
    /// than false, because the NULL element might have been a 2.
    ///
    /// A non-list right operand is `false` rather than an error, matching how
    /// the string operators above treat a type mismatch.
    fn eval_in(left: &Value, right: &Value) -> Value {
        if matches!(left, Value::Null(_)) {
            return Value::null();
        }
        let Value::List(list) = right else {
            return Value::Bool(false);
        };
        let mut saw_null = false;
        for item in &list.values {
            if Self::values_equal(left, item) {
                return Value::Bool(true);
            }
            saw_null |= matches!(item, Value::Null(_));
        }
        if saw_null {
            Value::null()
        } else {
            Value::Bool(false)
        }
    }

    fn values_equal(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Null(_), Value::Null(_)) => true,
            (Value::Null(_), _) | (_, Value::Null(_)) => false,
            (Value::Int(l), Value::Int(r)) => l == r,
            (Value::Float(l), Value::Float(r)) => (l - r).abs() < f64::EPSILON,
            (Value::Int(l), Value::Float(r)) => (*l as f64 - r).abs() < f64::EPSILON,
            (Value::Float(l), Value::Int(r)) => (l - *r as f64).abs() < f64::EPSILON,
            (Value::String(l), Value::String(r)) => l == r,
            (Value::Bool(l), Value::Bool(r)) => l == r,
            (Value::List(l), Value::List(r)) => {
                l.values.len() == r.values.len()
                    && l.values
                        .iter()
                        .zip(r.values.iter())
                        .all(|(a, b)| Self::values_equal(a, b))
            }
            _ => false,
        }
    }

    fn eval_compare<F>(left: Value, right: Value, cmp: F) -> Result<Value>
    where
        F: Fn(std::cmp::Ordering) -> bool,
    {
        let ordering = match (&left, &right) {
            (Value::Null(_), _) | (_, Value::Null(_)) => return Ok(Value::null()),
            (Value::Int(l), Value::Int(r)) => l.cmp(r),
            (Value::Float(l), Value::Float(r)) => {
                l.partial_cmp(r).unwrap_or(std::cmp::Ordering::Equal)
            }
            (Value::Int(l), Value::Float(r)) => (*l as f64)
                .partial_cmp(r)
                .unwrap_or(std::cmp::Ordering::Equal),
            (Value::Float(l), Value::Int(r)) => l
                .partial_cmp(&(*r as f64))
                .unwrap_or(std::cmp::Ordering::Equal),
            (Value::String(l), Value::String(r)) => l.cmp(r),
            _ => {
                return Err(ExecutionError::TypeMismatch(format!(
                    "Cannot compare {:?} and {:?}",
                    left, right
                )))
            }
        };

        Ok(Value::Bool(cmp(ordering)))
    }

    // Logical helpers
    fn eval_and(left: Value, right: Value) -> Result<Value> {
        match (left, right) {
            (Value::Bool(false), _) | (_, Value::Bool(false)) => Ok(Value::Bool(false)),
            (Value::Bool(true), Value::Bool(true)) => Ok(Value::Bool(true)),
            (Value::Null(_), Value::Bool(true)) | (Value::Bool(true), Value::Null(_)) => {
                Ok(Value::null())
            }
            (Value::Null(_), Value::Null(_)) => Ok(Value::null()),
            (l, r) => Err(ExecutionError::TypeMismatch(format!(
                "AND requires boolean operands, got {:?} and {:?}",
                l, r
            ))),
        }
    }

    fn eval_or(left: Value, right: Value) -> Result<Value> {
        match (left, right) {
            (Value::Bool(true), _) | (_, Value::Bool(true)) => Ok(Value::Bool(true)),
            (Value::Bool(false), Value::Bool(false)) => Ok(Value::Bool(false)),
            (Value::Null(_), Value::Bool(false)) | (Value::Bool(false), Value::Null(_)) => {
                Ok(Value::null())
            }
            (Value::Null(_), Value::Null(_)) => Ok(Value::null()),
            (l, r) => Err(ExecutionError::TypeMismatch(format!(
                "OR requires boolean operands, got {:?} and {:?}",
                l, r
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_literal_evaluation() {
        assert_eq!(
            Evaluator::evaluate(&Expression::Literal(Literal::Int(42))).unwrap(),
            Value::Int(42)
        );
    }

    #[test]
    fn test_comparison_operators() {
        // 10 < 20
        let expr = Expression::BinaryOp {
            op: BinaryOperator::Lt,
            left: Box::new(Expression::Literal(Literal::Int(10))),
            right: Box::new(Expression::Literal(Literal::Int(20))),
        };
        assert_eq!(Evaluator::evaluate(&expr).unwrap(), Value::Bool(true));
    }

    #[test]
    fn test_logical_operators() {
        // true AND false
        let expr = Expression::BinaryOp {
            op: BinaryOperator::And,
            left: Box::new(Expression::Literal(Literal::Bool(true))),
            right: Box::new(Expression::Literal(Literal::Bool(false))),
        };
        assert_eq!(Evaluator::evaluate(&expr).unwrap(), Value::Bool(false));
    }

    #[test]
    fn test_context_property_access() {
        let mut ctx = EvalContext::new();
        let mut props = HashMap::new();
        props.insert("person.age".to_string(), Value::Int(30));
        ctx.current = props;

        // person.age > 25
        let expr = Expression::BinaryOp {
            op: BinaryOperator::Gt,
            left: Box::new(Expression::Identifier("person.age".to_string())),
            right: Box::new(Expression::Literal(Literal::Int(25))),
        };
        assert_eq!(
            Evaluator::evaluate_with_context(&expr, &ctx).unwrap(),
            Value::Bool(true)
        );
    }

    // ----- is_a (R-3b ontology class membership) -----

    fn is_a(class: &str) -> Expression {
        Expression::FunctionCall {
            name: "is_a".to_string(),
            args: vec![Expression::Literal(Literal::String(class.to_string()))],
        }
    }

    fn ctx_with_isa(classes: &[&str]) -> EvalContext {
        let list = byoridb_common::datatypes::list::List::from(
            classes
                .iter()
                .map(|c| Value::String(c.to_string()))
                .collect::<Vec<_>>(),
        );
        let mut props = HashMap::new();
        props.insert("__isa__".to_string(), Value::List(list));
        EvalContext::new().with_current(props)
    }

    #[test]
    fn is_a_true_when_class_in_isa_set() {
        let ctx = ctx_with_isa(&["dog", "animal"]);
        assert_eq!(
            Evaluator::evaluate_with_context(&is_a("animal"), &ctx).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn is_a_false_when_class_absent() {
        let ctx = ctx_with_isa(&["dog", "animal"]);
        assert_eq!(
            Evaluator::evaluate_with_context(&is_a("plant"), &ctx).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn is_a_false_when_no_isa_injected() {
        // GO/MATCH/LOOKUP contexts never inject `__isa__` → is_a is false, not error.
        let ctx = EvalContext::new();
        assert_eq!(
            Evaluator::evaluate_with_context(&is_a("animal"), &ctx).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn is_a_null_arg_yields_null() {
        let ctx = ctx_with_isa(&["dog"]);
        let expr = Expression::FunctionCall {
            name: "is_a".to_string(),
            args: vec![Expression::Literal(Literal::Null)],
        };
        assert!(matches!(
            Evaluator::evaluate_with_context(&expr, &ctx).unwrap(),
            Value::Null(_)
        ));
    }

    #[test]
    fn is_a_non_string_arg_errors() {
        let ctx = ctx_with_isa(&["dog"]);
        let expr = Expression::FunctionCall {
            name: "is_a".to_string(),
            args: vec![Expression::Literal(Literal::Int(1))],
        };
        assert!(Evaluator::evaluate_with_context(&expr, &ctx).is_err());
    }

    /// `left <op> right` where both sides are literals or list literals.
    fn eval_op(left: Literal, op: BinaryOperator, items: Vec<Literal>) -> Value {
        let expr = Expression::BinaryOp {
            op,
            left: Box::new(Expression::Literal(left)),
            right: Box::new(Expression::List(
                items.into_iter().map(Expression::Literal).collect(),
            )),
        };
        Evaluator::evaluate_with_context(&expr, &EvalContext::new()).unwrap()
    }

    #[test]
    fn in_finds_a_matching_element() {
        assert_eq!(
            eval_op(
                Literal::Int(2),
                BinaryOperator::In,
                vec![Literal::Int(1), Literal::Int(2)]
            ),
            Value::Bool(true)
        );
    }

    #[test]
    fn in_is_false_when_no_element_matches() {
        assert_eq!(
            eval_op(
                Literal::Int(9),
                BinaryOperator::In,
                vec![Literal::Int(1), Literal::Int(2)]
            ),
            Value::Bool(false)
        );
    }

    #[test]
    fn in_an_empty_list_is_false() {
        assert_eq!(
            eval_op(Literal::Int(1), BinaryOperator::In, vec![]),
            Value::Bool(false)
        );
    }

    #[test]
    fn in_compares_int_and_float_the_way_equality_does() {
        assert_eq!(
            eval_op(
                Literal::Int(1),
                BinaryOperator::In,
                vec![Literal::Float(1.0)]
            ),
            Value::Bool(true)
        );
    }

    #[test]
    fn in_matches_strings() {
        assert_eq!(
            eval_op(
                Literal::String("grace".into()),
                BinaryOperator::In,
                vec![
                    Literal::String("ada".into()),
                    Literal::String("grace".into())
                ]
            ),
            Value::Bool(true)
        );
    }

    #[test]
    fn null_in_anything_is_unknown() {
        assert!(matches!(
            eval_op(Literal::Null, BinaryOperator::In, vec![Literal::Int(1)]),
            Value::Null(_)
        ));
    }

    #[test]
    fn a_null_element_makes_a_miss_unknown_but_not_a_hit() {
        // 2 IN [1, NULL] is unknown: the NULL might have been a 2.
        assert!(matches!(
            eval_op(
                Literal::Int(2),
                BinaryOperator::In,
                vec![Literal::Int(1), Literal::Null]
            ),
            Value::Null(_)
        ));
        // 1 IN [1, NULL] is true regardless: a match outranks the unknown.
        assert_eq!(
            eval_op(
                Literal::Int(1),
                BinaryOperator::In,
                vec![Literal::Int(1), Literal::Null]
            ),
            Value::Bool(true)
        );
    }

    #[test]
    fn not_in_negates_a_known_result_and_preserves_unknown() {
        assert_eq!(
            eval_op(
                Literal::Int(9),
                BinaryOperator::NotIn,
                vec![Literal::Int(1)]
            ),
            Value::Bool(true)
        );
        assert_eq!(
            eval_op(
                Literal::Int(1),
                BinaryOperator::NotIn,
                vec![Literal::Int(1)]
            ),
            Value::Bool(false)
        );
        assert!(matches!(
            eval_op(
                Literal::Int(2),
                BinaryOperator::NotIn,
                vec![Literal::Int(1), Literal::Null]
            ),
            Value::Null(_)
        ));
    }

    #[test]
    fn in_a_non_list_is_false_like_other_type_mismatches() {
        let expr = Expression::BinaryOp {
            op: BinaryOperator::In,
            left: Box::new(Expression::Literal(Literal::Int(1))),
            right: Box::new(Expression::Literal(Literal::Int(1))),
        };
        assert_eq!(
            Evaluator::evaluate_with_context(&expr, &EvalContext::new()).unwrap(),
            Value::Bool(false)
        );
    }

    /// `SCALAR_FUNCTIONS` exists so callers can ask whether a name is supported
    /// without evaluating it. That is only true while it matches what the
    /// dispatcher actually accepts, so assert it rather than trust it: every
    /// listed name must resolve to something other than "unknown function".
    #[test]
    fn every_listed_scalar_function_is_dispatched() {
        for name in SCALAR_FUNCTIONS {
            // One string argument satisfies most of them; the ones it does not
            // fit still fail on the argument rather than on the name, which is
            // what this test distinguishes.
            let outcome = Evaluator::apply_scalar_function(
                name,
                &[
                    Value::String("value".to_string()),
                    Value::String("v".to_string()),
                ],
            );
            if let Err(error) = &outcome {
                assert!(
                    !error.to_string().contains("Unknown function"),
                    "{name} is listed in SCALAR_FUNCTIONS but the dispatcher does not know it"
                );
            }
        }
    }

    /// The complement: an unlisted name must be reported as unknown, naming
    /// itself, because that error is what the MATCH planner turns into a query
    /// error instead of a silent NULL (#102).
    #[test]
    fn an_unlisted_function_is_reported_as_unknown_by_name() {
        let error = Evaluator::apply_scalar_function("frobnicate", &[Value::Int(1)])
            .expect_err("an unimplemented function must not produce a value");
        assert!(error.to_string().contains("Unknown function"), "{error}");
        assert!(error.to_string().contains("frobnicate"), "{error}");
        assert!(!SCALAR_FUNCTIONS.contains(&"FROBNICATE"));
    }
}
