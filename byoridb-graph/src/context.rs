/// Context for query execution
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub session_id: i64,
    pub space_name: Option<String>,
    pub caller_roles: Vec<String>,
}

impl ExecutionContext {
    pub fn new(session_id: i64) -> Self {
        Self {
            session_id,
            space_name: None,
            caller_roles: vec![],
        }
    }

    pub fn with_space(mut self, space: String) -> Self {
        self.space_name = Some(space);
        self
    }

    pub fn with_caller_roles(mut self, roles: Vec<String>) -> Self {
        self.caller_roles = roles;
        self
    }
}
