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
                for arg in &args {
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
}
