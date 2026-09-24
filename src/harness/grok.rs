use super::{add_passthrough, plain_model, resume_is_latest, Harness, Invocation, Request};
use crate::policy::RuntimePolicy;

pub(crate) fn new() -> Box<dyn Harness> {
    Box::new(GrokHarness)
}

struct GrokHarness;

impl Harness for GrokHarness {
    fn build(&self, request: &Request) -> Result<Invocation, String> {
        let policy = RuntimePolicy::from_env()?;
        Ok(Invocation::new(
            "grok",
            build_args(
                request,
                policy.grok_denies(),
                policy.grok_allows(),
                policy.grok_subagents_disabled(),
            ),
        ))
    }
}

fn build_args(
    request: &Request,
    denies: &[String],
    allows: &[String],
    disable_subagents: bool,
) -> Vec<String> {
    let mut args = vec!["--no-auto-update".to_string()];
    if disable_subagents {
        args.push("--no-subagents".to_string());
    }
    if let Some(prompt) = &request.prompt {
        args.extend(["--single".to_string(), prompt.clone()]);
        args.extend([
            "--output-format".to_string(),
            request
                .output_format
                .clone()
                .unwrap_or_else(|| "plain".to_string()),
        ]);
    }
    if let Some(model) = plain_model(request) {
        args.extend(["--model".to_string(), model]);
    }
    if let Some(effort) = &request.reasoning_effort {
        args.extend(["--reasoning-effort".to_string(), effort.clone()]);
    }
    if let Some(mode) = &request.permission_mode {
        args.extend(["--permission-mode".to_string(), mode.clone()]);
    }
    if let Some(turns) = &request.max_turns {
        args.extend(["--max-turns".to_string(), turns.clone()]);
    }
    if let Some(resume) = &request.resume_id {
        if resume_is_latest(resume) {
            args.push("--continue".to_string());
        } else {
            args.extend(["--resume".to_string(), resume.clone()]);
        }
    } else if let Some(session) = &request.session_id {
        args.extend(["--session-id".to_string(), session.clone()]);
    }
    if request.yolo {
        args.push("--always-approve".to_string());
    }

    for rule in denies {
        args.extend(["--deny".to_string(), rule.clone()]);
    }
    for rule in allows {
        args.extend(["--allow".to_string(), rule.clone()]);
    }
    add_passthrough(args, request)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        Request {
            harness: "grok".to_string(),
            provider: None,
            model: None,
            reasoning_effort: None,
            output_format: None,
            input_format: None,
            permission_mode: None,
            max_turns: None,
            agent: None,
            cwd: None,
            prompt: None,
            passthrough: Vec::new(),
            dry_run: false,
            yolo: false,
            session_id: None,
            resume_id: None,
        }
    }

    #[test]
    fn maps_headless_session_and_policy_arguments() {
        let mut req = request();
        req.prompt = Some("review this".to_string());
        req.model = Some("grok-code".to_string());
        req.reasoning_effort = Some("xhigh".to_string());
        req.permission_mode = Some("auto".to_string());
        req.max_turns = Some("30".to_string());
        req.session_id = Some("01a06582-d66e-7811-b0c9-0b0266e17903".to_string());
        let args = build_args(
            &req,
            &["WebSearch".to_string()],
            &["Bash(*parley-health-query.exe*)".to_string()],
            true,
        );
        assert_eq!(
            args,
            vec![
                "--no-auto-update",
                "--no-subagents",
                "--single",
                "review this",
                "--output-format",
                "plain",
                "--model",
                "grok-code",
                "--reasoning-effort",
                "xhigh",
                "--permission-mode",
                "auto",
                "--max-turns",
                "30",
                "--session-id",
                "01a06582-d66e-7811-b0c9-0b0266e17903",
                "--deny",
                "WebSearch",
                "--allow",
                "Bash(*parley-health-query.exe*)",
            ]
        );
    }

    #[test]
    fn omits_max_turns_when_unset() {
        let mut req = request();
        req.prompt = Some("review this".to_string());
        req.session_id = Some("01a06582-d66e-7811-b0c9-0b0266e17903".to_string());
        let args = build_args(&req, &[], &[], true);
        assert!(!args.iter().any(|argument| argument == "--max-turns"));
    }

    #[test]
    fn maps_resume_latest_yolo_and_passthrough() {
        let mut req = request();
        req.prompt = Some("continue".to_string());
        req.resume_id = Some("latest".to_string());
        req.yolo = true;
        req.passthrough = vec!["--verbose".to_string()];
        let args = build_args(&req, &[], &[], false);
        assert!(args.iter().any(|arg| arg == "--continue"));
        assert!(args.iter().any(|arg| arg == "--always-approve"));
        assert!(args.ends_with(&["--verbose".to_string()]));
    }
}
