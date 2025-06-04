use rmcp::{
    model::{
        CallToolResult, Content, Implementation, InitializeRequestParam, InitializeResult,
        ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    tool,
};
use std::collections::HashMap;

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
    async fn format_project(&self) -> Result<CallToolResult, rmcp::Error> {
        let mut cmd = build_fmt_command(&self.movefmt_cmd);
        cmd.arg(&format!("{}/sources", &self.project_folder))
            .output()
            .map_err(|e| {
                rmcp::Error::internal_error(
                    format!("Failed to run formatter on `sources`: {}", e),
                    None,
                )
            })?;
        cmd.arg(&format!("{}/tests", &self.project_folder))
            .output()
            .map_err(|e| {
                rmcp::Error::internal_error(
                    format!("Failed to run formatter on `tests`: {}", e),
                    None,
                )
            })?;
        Ok(CallToolResult::success(vec![Content::text("OK")]))
    }

    #[tool(description = "Builds the project and runs tests")]
    async fn validate_project(&self) -> Result<CallToolResult, rmcp::Error> {
        let build_output = std::process::Command::new("sui")
            .arg("move")
            .arg("build")
            .arg("--force")
            .current_dir(&self.project_folder)
            .output()
            .map_err(|e| {
                rmcp::Error::internal_error(format!("Failed to build project: {}", e), None)
            })?;

        let output_data = String::from_utf8_lossy(&build_output.stderr);

        let (build_warnings, build_errors) = extract_build_output(&output_data);

        if !build_errors.is_empty() {
            let body = serde_json::json!({
                "warnings": build_warnings.values().collect::<Vec<_>>(),
                "buildErrors": build_errors.values().collect::<Vec<_>>(),
                "testResults": null
            });
            let out = Content::json(body)?;
            return Ok(CallToolResult::success(vec![out]));
        }

        let output = std::process::Command::new("sui")
            .arg("move")
            .arg("test")
            // JSON output provides insufficient information
            // https://github.com/MystenLabs/sui/blob/5f28d37e21e4064a99bb2fff08210c8a62fbbb94/external-crates/move/crates/move-compiler/src/diagnostics/mod.rs#L86
            //.arg("--json-errors")
            .current_dir(&self.project_folder)
            .output()
            .map_err(|e| {
                rmcp::Error::internal_error(format!("Failed to run tests: {}", e), None)
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let test_results = if stdout.contains("Test failures") {
            let data = parse_test_output(&stdout);
            Some(format!("FAILED:\n\n{}", data.trim()))
        } else if stdout.contains("Test result: OK") {
            Some("PASSED".to_string())
        } else {
            None
        };

        let (mut test_warnings, test_errors) = extract_build_output(&stderr);
        test_warnings.extend(build_warnings);

        let body = serde_json::json!({
            "warnings": test_warnings.values().collect::<Vec<_>>(),
            "buildErrors": test_errors.values().collect::<Vec<_>>(),
            "testResults": test_results
        });
        let out = Content::json(body)?;
        Ok(CallToolResult::success(vec![out]))
    }
}

#[tool(tool_box)]
impl rmcp::ServerHandler for SuiService {
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
        context: RequestContext<rmcp::RoleServer>,
    ) -> Result<InitializeResult, rmcp::Error> {
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

#[derive(Hash, Eq, PartialEq, Debug)]
pub struct LineNotice {
    file: String,
    line_number: u32,
    column_number: u32,
    code: String,
}

pub fn extract_build_output(
    input: &str,
) -> (HashMap<LineNotice, String>, HashMap<LineNotice, String>) {
    let mut warnings = HashMap::new();
    let mut errors = HashMap::new();

    let v = strip_ansi_escapes::strip(input);
    let s = String::from_utf8_lossy(&v);
    let mut lines = s.lines().peekable();

    while let Some(line) = lines.next() {
        if line.starts_with("warning[") {
            let mut warning_block = String::new();
            warning_block.push_str(line);
            warning_block.push('\n');

            let code = line.split(']').next().unwrap().replace("warning[", "");
            let mut location = None;

            while let Some(next_line) = lines.peek() {
                if next_line.trim().starts_with("=") {
                    lines.next(); // Consume the '=' line
                    break;
                }
                if location.is_none() {
                    location = parse_location(next_line);
                }
                warning_block.push_str(lines.next().unwrap());
                warning_block.push('\n');
            }

            let (file, line_number, column_number) = location.expect("warning block fail");

            let notice = LineNotice {
                file,
                line_number,
                code,
                column_number,
            };
            warnings.insert(notice, warning_block.trim().to_string());
        } else if line.starts_with("error[") {
            let mut error_block = String::new();
            error_block.push_str(line);
            error_block.push('\n');

            let code = line.split(']').next().unwrap().replace("error[", "");
            let mut location = None;

            while let Some(next_line) = lines.peek() {
                if next_line.is_empty() {
                    lines.next(); // Consume the empty line
                    break;
                }
                if location.is_none() {
                    location = parse_location(next_line);
                }
                error_block.push_str(lines.next().unwrap());
                error_block.push('\n');
            }
            let (file, line_number, column_number) = location.expect("error block fail");
            let notice = LineNotice {
                file,
                line_number,
                code,
                column_number,
            };
            errors.insert(notice, error_block.trim().to_string());
        }
    }

    (warnings, errors)
}

fn parse_location(val: &str) -> Option<(String, u32, u32)> {
    if val.trim().starts_with("┌─") {
        let parts: Vec<&str> = val.split(':').collect();
        if parts.len() >= 3 {
            Some((
                parts.get(0)?.replace("┌─", "").trim().to_string(),
                parts.get(1)?.parse().ok()?,
                parts.get(2)?.parse().ok()?,
            ))
        } else {
            None
        }
    } else {
        None
    }
}
