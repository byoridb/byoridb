// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use crate::datatypes::value::Value;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DataSet {
    pub column_names: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

impl DataSet {
    pub fn new(column_names: Vec<String>) -> Self {
        DataSet {
            column_names,
            rows: Vec::new(),
        }
    }

    pub fn with_rows(column_names: Vec<String>, rows: Vec<Vec<Value>>) -> Self {
        DataSet { column_names, rows }
    }

    pub fn add_row(&mut self, row: Vec<Value>) {
        self.rows.push(row);
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn col_count(&self) -> usize {
        self.column_names.len()
    }

    pub fn to_string(&self) -> String {
        let mut result = format!("Columns: {:?}\n", self.column_names);
        for row in &self.rows {
            let row_str: Vec<String> = row.iter().map(|v| v.to_string()).collect();
            result.push_str(&format!("{}\n", row_str.join(", ")));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_starts_empty() {
        let ds = DataSet::new(vec!["a".into(), "b".into()]);
        assert_eq!(ds.col_count(), 2);
        assert_eq!(ds.row_count(), 0);
    }

    #[test]
    fn test_add_row_increments_count() {
        let mut ds = DataSet::new(vec!["x".into()]);
        ds.add_row(vec![Value::Int(1)]);
        ds.add_row(vec![Value::Int(2)]);
        assert_eq!(ds.row_count(), 2);
    }

    #[test]
    fn test_with_rows_preserves_data() {
        let ds = DataSet::with_rows(
            vec!["a".into(), "b".into()],
            vec![vec![Value::Int(1), Value::Int(2)]],
        );
        assert_eq!(ds.row_count(), 1);
        assert_eq!(ds.rows[0][0], Value::Int(1));
    }

    #[test]
    fn test_to_string_includes_columns_and_rows() {
        let ds = DataSet::with_rows(
            vec!["name".into(), "age".into()],
            vec![vec![Value::String("Alice".into()), Value::Int(30)]],
        );
        let s = ds.to_string();
        assert!(s.contains("name"));
        assert!(s.contains("age"));
        assert!(s.contains("Alice"));
        assert!(s.contains("30"));
    }

    #[test]
    fn test_to_string_empty_dataset() {
        let ds = DataSet::new(vec!["c".into()]);
        let s = ds.to_string();
        assert!(s.contains("Columns:"));
        assert!(s.contains("c"));
    }
}
