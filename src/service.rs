use rmcp::{
    model::{
        CallToolResult, Content, Implementation, InitializeRequestParam, InitializeResult,
        ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    tool, Error as McpError, RoleServer, ServerHandler,
};

#[derive(Clone)]
pub struct SuiService {
    project_folder: String,
    movefmt_cmd: String,
}

#[tool(tool_box)]
impl SuiService {
    pub fn new(project_folder: &str, movefmt_cmd: &str) -> Self {
        Self {
            project_folder: project_folder.to_string(),
            movefmt_cmd: movefmt_cmd.to_string(),
        }
    }

    #[tool(description = "Format project")]
    async fn format_project(&self) -> Result<CallToolResult, McpError> {
        let mut cmd = build_fmt_command(&self.movefmt_cmd);
        cmd.arg(&format!("{}/sources", &self.project_folder))
            .output()
            .map_err(|e| {
                McpError::internal_error(
                    format!("Failed to run formatter on `sources`: {}", e),
                    None,
                )
            })?;
        cmd.arg(&format!("{}/tests", &self.project_folder))
            .output()
            .map_err(|e| {
                McpError::internal_error(format!("Failed to run formatter on `tests`: {}", e), None)
            })?;
        Ok(CallToolResult::success(vec![Content::text("OK")]))
    }

    #[tool(description = "Builds the project and runs tests")]
    async fn validate_project(&self) -> Result<CallToolResult, McpError> {
        let output = std::process::Command::new("sui")
            .arg("move")
            .arg("test")
            // JSON output provides insufficient information
            // https://github.com/MystenLabs/sui/blob/5f28d37e21e4064a99bb2fff08210c8a62fbbb94/external-crates/move/crates/move-compiler/src/diagnostics/mod.rs#L86
            //.arg("--json-errors")
            .arg("--force")
            .current_dir(&self.project_folder)
            .output()
            .map_err(|e| {
                McpError::internal_error(
                    format!("Failed to run formatter on `sources`: {}", e),
                    None,
                )
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let err = String::from_utf8_lossy(&output.stderr);

        let (ws, es) = extract_build_output(&err);

        let test_results = if stdout.contains("Test failures") {
            let data = parse_test_output(&stdout);
            Some(format!("FAILED:\n\n{}", data.trim()))
        } else if stdout.contains("Test result: OK") {
            Some("PASSED".to_string())
        } else {
            None
        };

        let body = serde_json::json!({
            "warnings": ws,
            "buildErrors": es,
            "testResults": test_results
        });
        let out = Content::json(body)?;
        Ok(CallToolResult::success(vec![out]))
    }
}

#[tool(tool_box)]
impl ServerHandler for SuiService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "This server provides tools to help manage a Sui Move project.".to_string(),
            ),
        }
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        if let Some(http_request_part) = context.extensions.get::<axum::http::request::Parts>() {
            let initialize_headers = &http_request_part.headers;
            let initialize_uri = &http_request_part.uri;
            tracing::info!(?initialize_headers, %initialize_uri, "initialize from http server");
        }
        Ok(self.get_info())
    }
}

fn build_fmt_command(cmd_str: &str) -> std::process::Command {
    let mut parts = cmd_str.split(' ');
    let mut cmd = std::process::Command::new(parts.next().unwrap());
    for part in parts {
        cmd.arg(part);
    }
    cmd
}

fn parse_test_output(s: &str) -> String {
    remove_before(s, "Test failures")
}

fn remove_before(s: &str, pattern: &str) -> String {
    s.find(pattern)
        .map(|idx| &s[idx..])
        .unwrap_or(s)
        .to_string()
}

fn extract_build_output(input: &str) -> (Vec<String>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut errors = Vec::new();

    let v = strip_ansi_escapes::strip(input);
    let s = String::from_utf8_lossy(&v);
    let mut lines = s.lines().peekable();

    while let Some(line) = lines.next() {
        if line.starts_with("warning[") {
            let mut warning_block = String::new();
            warning_block.push_str(line);
            warning_block.push('\n');

            while let Some(next_line) = lines.peek() {
                if next_line.starts_with("  =") {
                    lines.next(); // Consume the '=' line
                    break;
                }
                warning_block.push_str(lines.next().unwrap());
                warning_block.push('\n');
            }
            warnings.push(warning_block.trim().to_string());
        } else if line.starts_with("error[") {
            let mut error_block = String::new();
            error_block.push_str(line);
            error_block.push('\n');

            while let Some(next_line) = lines.peek() {
                if next_line.is_empty() {
                    lines.next(); // Consume the empty line
                    break;
                }
                error_block.push_str(lines.next().unwrap());
                error_block.push('\n');
            }
            errors.push(error_block.trim().to_string());
        }
    }

    (warnings, errors)
}
