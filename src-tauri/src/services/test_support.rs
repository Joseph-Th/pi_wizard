use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use pi_wizard_core::RunId;
use pi_wizard_core::environment::{
    LaunchEnvironmentInput, ResolvedLaunchEnvironment, resolve_launch_environment,
};

pub(crate) struct WorkflowFakePiFixture {
    pub(crate) root: PathBuf,
    fake_pi: PathBuf,
}

impl WorkflowFakePiFixture {
    pub(crate) fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("pi-wizard-{name}-{}", RunId::new()));
        fs::create_dir_all(&root).expect("create workflow fake Pi fixture");
        #[cfg(windows)]
        let script = root
            .join("node_modules")
            .join("@earendil-works")
            .join("pi-coding-agent")
            .join("dist")
            .join("bundle")
            .join("cli.js");
        #[cfg(not(windows))]
        let script = root.join("workflow-fake-pi.js");
        fs::create_dir_all(script.parent().expect("workflow fake Pi script parent"))
            .expect("create workflow fake Pi script parent");
        fs::write(&script, WORKFLOW_FAKE_PI_JS).expect("write workflow fake Pi script");

        #[cfg(windows)]
        let fake_pi = {
            let path = root.join("pi.cmd");
            fs::write(
                &path,
                "@echo off\r\nnode \"%~dp0node_modules\\@earendil-works\\pi-coding-agent\\dist\\bundle\\cli.js\" %*\r\n",
            )
            .expect("write wrapped Pi shim");
            path
        };

        #[cfg(not(windows))]
        let fake_pi = {
            use std::os::unix::fs::PermissionsExt;
            let path = root.join("pi");
            fs::write(
                &path,
                "#!/bin/sh\nexec node \"$(dirname \"$0\")/workflow-fake-pi.js\" \"$@\"\n",
            )
            .expect("write Unix workflow Pi wrapper");
            let mut permissions = fs::metadata(&path).expect("wrapper metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).expect("wrapper permissions");
            path
        };

        Self { root, fake_pi }
    }

    pub(crate) fn environment(&self) -> ResolvedLaunchEnvironment {
        let desktop_environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        resolve_launch_environment(LaunchEnvironmentInput {
            configured_pi: Some(self.fake_pi.clone()),
            desktop_environment,
            ..LaunchEnvironmentInput::default()
        })
        .expect("resolve workflow fake Pi environment")
    }

    pub(crate) fn initialize_git_repository(&self) -> ResolvedLaunchEnvironment {
        let environment = self.environment();
        let git = environment
            .git_executable()
            .expect("Git is required for workflow worktree integration");
        let run_git = |args: &[&str]| {
            let status = Command::new(git)
                .current_dir(&self.root)
                .args(args)
                .status()
                .expect("run Git workflow fixture command");
            assert!(status.success(), "Git fixture command failed: {args:?}");
        };
        run_git(&["init"]);
        run_git(&["config", "user.email", "pi-wizard-tests@example.invalid"]);
        run_git(&["config", "user.name", "Pi Wizard Tests"]);
        run_git(&["config", "core.autocrlf", "false"]);
        fs::write(self.root.join("seed.txt"), "workflow worktree fixture\n")
            .expect("write Git seed file");
        run_git(&["add", "."]);
        run_git(&["commit", "-m", "workflow fixture base"]);
        environment
    }

    pub(crate) fn worktree_parent(&self) -> PathBuf {
        let repository_name = self
            .root
            .file_name()
            .expect("fixture repository name")
            .to_string_lossy();
        self.root
            .parent()
            .expect("fixture repository parent")
            .join(format!("{repository_name}-worktrees"))
    }
}

impl Drop for WorkflowFakePiFixture {
    fn drop(&mut self) {
        let worktrees = self.worktree_parent();
        if worktrees.exists() {
            let _ = fs::remove_dir_all(worktrees);
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

const WORKFLOW_FAKE_PI_JS: &str = r#"
const fs = require("fs");
let buffer = "";
let working = false;
let assistantMessages = 0;
let lastAssistantText = "";
let supervisorTurns = 0;
let sessionId = `workflow-${process.pid}`;
let delayNextStateAfterSwitch = false;
const supervisorProcess = process.argv.includes("--no-context-files") && process.argv.includes("--no-extensions");
const delayedSupervisorStartup = supervisorProcess && fs.existsSync("workflow-delay-supervisor-startup");

function emit(value) {
  process.stdout.write(JSON.stringify(value) + "\n");
}

function respond(request, data) {
  const value = {
    id: request.id,
    type: "response",
    command: request.type,
    success: true,
  };
  if (data !== undefined) value.data = data;
  emit(value);
}

function reject(request, error) {
  emit({
    id: request.id,
    type: "response",
    command: request.type,
    success: false,
    error,
  });
}

function state() {
  return {
    model: {
      provider: "fake",
      id: "fake-model",
      name: "Fake Model",
      input: ["text"],
    },
    thinkingLevel: "medium",
    isStreaming: working,
    isCompacting: false,
    steeringMode: "all",
    followUpMode: "one-at-a-time",
    sessionFile: null,
    sessionId,
    sessionName: null,
    autoCompactionEnabled: true,
    messageCount: assistantMessages,
    pendingMessageCount: 0,
  };
}

function handle(request) {
  switch (request.type) {
    case "get_state":
      if (delayNextStateAfterSwitch) {
        delayNextStateAfterSwitch = false;
        setTimeout(() => respond(request, state()), 900);
      } else if (delayedSupervisorStartup) {
        setTimeout(() => respond(request, state()), 5_000);
      } else {
        respond(request, state());
      }
      break;
    case "get_available_models":
      respond(request, {
        models: [
          {provider: "fake", id: "fake-model", name: "Fake Model", input: ["text"]},
          {provider: "fake", id: "alternate-model", name: "Alternate Model", input: ["text", "image"]},
          {provider: "other-provider", id: "other-model", name: "Other Model", input: ["text"]},
        ],
      });
      break;
    case "get_available_thinking_levels":
      respond(request, {levels: ["off", "medium", "high", "xhigh"]});
      break;
    case "get_commands":
      respond(request, {commands: []});
      break;
    case "get_session_stats":
      fs.appendFileSync("workflow-session-stats.log", "probe\n");
      respond(request, {
        sessionFile: "",
        sessionId,
        userMessages: assistantMessages,
        assistantMessages,
        toolCalls: 0,
        toolResults: 0,
        totalMessages: assistantMessages * 2,
        tokens: {input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0},
        cost: 0,
        contextUsage: null,
      });
      break;
    case "get_last_assistant_text":
      respond(request, {text: lastAssistantText});
      break;
    case "export_html":
      respond(request, {path: "session-export.html"});
      break;
    case "bash":
      if (request.command === "slow-bash") {
        emit({type: "bash_execution_update", id: request.id, delta: "slow bash active"});
        setTimeout(() => respond(request, {
          output: "slow bash done",
          exitCode: 0,
          cancelled: false,
          truncated: false,
        }), 1_000);
      } else {
        respond(request, {
          output: `fake output: ${request.command}`,
          exitCode: 0,
          cancelled: false,
          truncated: false,
        });
      }
      break;
    case "abort_bash":
      respond(request);
      break;
    case "switch_session":
      sessionId = `workflow-switched-${process.pid}`;
      assistantMessages = 0;
      lastAssistantText = "";
      delayNextStateAfterSwitch = true;
      respond(request, {cancelled: false});
      break;
    case "prompt":
      if (request.message === "reject this step") {
        reject(request, "fixture prompt rejection");
        break;
      }
      respond(request);
      working = true;
      emit({type: "agent_start"});
      if (supervisorProcess) {
        const matches = [...String(request.message).matchAll(/runId=([0-9a-f-]{36}) projectId=[0-9a-f-]{36} decisionRequired=true status=idle/g)];
        const stopDuringBashRace = String(request.message).includes("STOP_DURING_BASH_RACE");
        if (supervisorTurns === 0 && matches.length > 0) {
          lastAssistantText = JSON.stringify({
            directives: matches.map((match) => stopDuringBashRace
              ? {runId: match[1], action: "stop"}
              : {
                  runId: match[1],
                  action: "send",
                  message: "supervised continuation",
                }),
          });
        } else {
          lastAssistantText = JSON.stringify({directives: []});
        }
        supervisorTurns += 1;
      } else {
        fs.appendFileSync("workflow-worker-prompts.log", String(request.message) + "\n");
        lastAssistantText = `done: ${request.message}`;
      }
      setTimeout(() => {
        emit({
          type: "message_end",
          message: {
            role: "assistant",
            content: [{type: "text", text: lastAssistantText}],
          },
        });
        assistantMessages += 1;
        working = false;
        emit({type: "agent_settled"});
      }, supervisorProcess && String(request.message).includes("RACE_TEST_DELAY")
        ? 500
        : String(request.message).startsWith("race ")
          ? 1000
          : String(request.message).startsWith("slow ")
            ? 200
            : 20);
      break;
    case "steer":
    case "follow_up":
      respond(request);
      break;
    case "abort":
    case "abort_retry":
      respond(request);
      working = false;
      emit({type: "agent_settled"});
      break;
    default:
      respond(request);
      break;
  }
}

process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  while (true) {
    const newline = buffer.indexOf("\n");
    if (newline < 0) break;
    const line = buffer.slice(0, newline).replace(/\r$/, "");
    buffer = buffer.slice(newline + 1);
    if (!line) continue;
    handle(JSON.parse(line));
  }
});
"#;
