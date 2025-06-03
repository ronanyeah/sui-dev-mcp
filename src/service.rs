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
            .arg("--json-errors")
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
        if stdout.contains("Test failures") {
            Ok(CallToolResult::success(vec![Content::text(stdout)]))
        } else if err.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(
                "OK".to_string(),
            )]))
        } else {
            let err_data: serde_json::Value = serde_json::from_str(&err)
                .map_err(|_| McpError::internal_error("Sui error serialize fail", None))?;
            let out = Content::json(err_data)?;
            Ok(CallToolResult::success(vec![out]))
        }
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
