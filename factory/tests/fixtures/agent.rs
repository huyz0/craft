use std::io::Read;

fn main() {
    if std::env::args().skip(1).eq(["login", "status"]) {
        let home = std::path::PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        if home.join("hang").exists() {
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
        assert!(home.join("logged-in").is_file());
        println!("Synthetic authentication probe output");
        return;
    }
    if std::env::args().skip(1).eq(["auth", "status", "--hostname", "github.com"]) {
        eprintln!("Synthetic GitHub authentication probe output");
        return;
    }
    let planning = std::env::args().any(|arg| arg == "--ignore-rules");
    if planning {
        let args: Vec<_> = std::env::args().collect();
        assert!(args.windows(2).any(|pair| pair == ["--sandbox", "read-only"]));
        assert!(args.windows(2).any(|pair| pair == ["--ask-for-approval", "never"]));
        for flag in ["--ignore-user-config", "--skip-git-repo-check", "--ephemeral"] {
            assert!(args.iter().any(|arg| arg == flag));
        }
        assert!(std::env::var_os("CRAFT_FACTORY_OPERATOR_TOKEN").is_none());
        assert!(std::env::var_os("BITBUCKET_TOKEN").is_none());
        assert!(std::fs::read_dir(".").unwrap().next().is_none());
        let mut prompt = String::new();
        std::io::stdin().read_to_string(&mut prompt).unwrap();
        assert!(prompt.contains("You are the release coordinator"));
        if prompt.contains("series-fixture") {
            assert!(prompt.contains("series-upstream") && prompt.contains("series-downstream") && prompt.contains("series-final"));
            println!(r#"{{"type":"item.completed","item":{{"type":"agent_message","text":"{{\"outcome\":\"proposed\",\"summary\":\"Stage the upstream change before the dependent updates.\",\"compatibility\":\"Preserve the upstream contract during the rollout.\",\"rollback\":\"Withdraw the failed release observation.\",\"members\":[{{\"work_id\":\"series-upstream\",\"intent\":\"Change the upstream greeting.\",\"acceptance\":[\"Change src/hello\"]}},{{\"work_id\":\"series-downstream\",\"intent\":\"Change the downstream greeting.\",\"acceptance\":[\"Change src/hello\"]}},{{\"work_id\":\"series-final\",\"intent\":\"Complete the follow-up change.\",\"acceptance\":[\"Change src/hello\"]}}]}}"}}}}"#);
            println!(r#"{{"type":"turn.completed","usage":{{"input_tokens":13,"cached_input_tokens":3,"output_tokens":12}}}}"#);
            return;
        }
        assert!(prompt.contains("api-add-optional-field") && prompt.contains("consumer-read-optional-field"));
        if prompt.contains("Coordinator timeout fixture") {
            std::thread::sleep(std::time::Duration::from_secs(30));
            return;
        }
        if prompt.contains("Coordinator profile fixture") {
            assert!(args.windows(2).any(|pair| pair == ["--model", "fixture-planning"]));
            assert!(args.windows(2).any(|pair| pair == ["-c", "model_reasoning_effort=\"high\""]));
        }
        if prompt.contains("Coordinator blocked fixture") {
            println!(r#"{{"type":"item.completed","item":{{"type":"agent_message","text":"{{\"outcome\":\"blocked\",\"summary\":\"Need the compatibility contract.\",\"compatibility\":\"\",\"rollback\":\"\",\"members\":[]}}"}}}}"#);
        } else {
            let first = if prompt.contains("Coordinator unknown ID fixture") { "invented-work" } else { "consumer-read-optional-field" };
            println!(r#"{{"type":"item.completed","item":{{"type":"agent_message","text":"{{\"outcome\":\"proposed\",\"summary\":\"Stage a tolerant reader first.\",\"compatibility\":\"Ship a tolerant reader before the API change.\",\"rollback\":\"Disable the feature flag.\",\"members\":[{{\"work_id\":\"{first}\",\"intent\":\"Accept both payload versions.\",\"acceptance\":[\"The client accepts both fixtures.\"]}},{{\"work_id\":\"api-add-optional-field\",\"intent\":\"Add the optional field.\",\"acceptance\":[\"The old contract still passes.\"]}}]}}"}}}}"#);
        }
        println!(r#"{{"type":"turn.completed","usage":{{"input_tokens":11,"cached_input_tokens":2,"output_tokens":9}}}}"#);
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("--kubeconfig") {
        use std::io::Write;
        let args: Vec<_> = std::env::args().collect();
        let kubeconfig = std::path::Path::new(&args[2]);
        let base = kubeconfig.parent().unwrap();
        assert!(kubeconfig.is_absolute());
        assert_eq!(&args[3..8], ["--context", "stage-context", "--request-timeout=5s", "get", "--raw"]);
        assert_eq!(std::env::var("KUBECTL_KUBERC").unwrap(), "false");
        assert!(std::env::var_os("CRAFT_FACTORY_RUNNER_TOKEN").is_none());
        let calls = base.join("calls");
        let observed = std::fs::read_to_string(&calls).unwrap_or_default().lines().count();
        let path = match args[8].as_str() {
            "/api/v1/namespaces/stage/pods/api-live" if observed == 2 && base.join("pod-next.json").exists() => "pod-next.json",
            "/api/v1/namespaces/stage/pods/api-live" => "pod.json",
            "/apis/apps/v1/namespaces/stage/replicasets/api-fixture" => "replica-set.json",
            other => panic!("unexpected capture route {}", other),
        };
        std::fs::OpenOptions::new().create(true).append(true).open(calls).unwrap().write_all(format!("{}\n", args[8]).as_bytes()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(80));
        std::io::stdout().write_all(&std::fs::read(base.join(path)).unwrap()).unwrap();
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("--restricted") {
        let args: Vec<_> = std::env::args().collect();
        for flag in ["--bare", "--print", "--no-session-persistence", "--strict-mcp-config"] {
            assert!(args.iter().any(|arg| arg == flag));
        }
        assert!(args.windows(2).any(|pair| pair == ["--permission-mode", "dontAsk"]));
        assert!(args.windows(2).any(|pair| pair == ["--tools", "Read,Edit,Write,Glob,Grep"]));
        assert_eq!(std::env::current_dir().unwrap(), std::path::Path::new("/work"));
        assert_eq!(std::env::var("ANTHROPIC_API_KEY").unwrap(), "fixture-key");
        assert!(std::env::var_os("CRAFT_FACTORY_RUNNER_TOKEN").is_none());
        assert!(std::env::var_os("FACTORY_TEST_SECRET").is_none());
        assert!(std::fs::write(".git/factory-probe", "must stay read-only").is_err());
        let mut prompt = String::new();
        std::io::stdin().read_to_string(&mut prompt).unwrap();
        assert!(prompt.contains("Pinned skill for fixture"));
        let outside = prompt.lines().find_map(|line| {
            line.split_once("\"intent\": \"Namespace sentinel ")
                .map(|(_, path)| path.trim_end_matches(['"', ',']))
        }).unwrap();
        assert!(std::fs::read_to_string(outside).is_err());
        if prompt.contains("Wait for lease loss") {
            std::fs::write("src/claude-waiting", "started").unwrap();
            std::thread::sleep(std::time::Duration::from_secs(30));
            return;
        }
        if prompt.contains("Reject structured output") {
            println!(r#"{{"type":"result","subtype":"success","is_error":false}}"#);
            return;
        }
        std::fs::write("src/hello", "changed by fixture Claude adapter").unwrap();
        let knowledge = if prompt.contains("one unapproved knowledge_proposal") {
            let schema = args.windows(2).find(|pair| pair[0] == "--json-schema").unwrap();
            assert!(schema[1].contains(r#""knowledge_proposal""#));
            r#","knowledge_proposal":{"kind":"runbook","location":"project:fixture-claude","body":"Read the scoped namespace fixture output.","evidence":"src/hello"}"#
        } else {
            ""
        };
        println!(r#"{{"type":"result","subtype":"success","is_error":false,"structured_output":{{"outcome":"candidate","summary":"The scoped edit is ready."{knowledge}}},"usage":{{"input_tokens":21,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":9}}}}"#);
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("confluence") {
        let args: Vec<_> = std::env::args().collect();
        assert_eq!(&args[1..4], ["confluence", "page", "get"]);
        assert!(args.windows(2).any(|pair| pair == ["--profile", "fixture-confluence"]));
        assert!(args.windows(2).any(|pair| pair == ["--fields", "space_id,title,status,version,revision"]));
        let site = &args[args.iter().position(|arg| arg == "--cloud-id").unwrap() + 1];
        let space = if std::env::var_os("FACTORY_FIXTURE_CONFLUENCE_WRONG_SPACE").is_some() { "201" } else { "200" };
        let version = if std::env::var_os("FACTORY_FIXTURE_CONFLUENCE_CHANGED").is_some() { 6 } else { 5 };
        assert!(std::env::var_os("CRAFT_FACTORY_OPERATOR_TOKEN").is_none());
        assert!(std::env::var_os("CREDENTIALS_DIRECTORY").is_none());
        if std::env::var_os("FACTORY_FIXTURE_CONFLUENCE_OVERSIZE").is_some() {
            println!("{}", "x".repeat(32768));
            return;
        }
        let digest = if std::env::var_os("FACTORY_FIXTURE_CONFLUENCE_DIFF_BODY").is_some() { "b" } else { "a" }.repeat(64);
        println!(r#"{{"schema_version":"1.0","ok":true,"command":"confluence.page.get","data":{{"target":{{"provider":"confluence","deployment":"cloud","tenant":"{site}","resource_type":"site","id":"{site}"}},"item":{{"id":"101","space_id":"{space}","title":"Reviewed routing decision","status":"current","version":{version},"revision":{{"version":{version},"digest":"sha256:{digest}","representation":"storage"}},"fields_not_returned":[]}}}}}}"#);
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("jira") {
        let args: Vec<_> = std::env::args().collect();
        assert_eq!(&args[1..3], ["jira", "issue"]);
        assert!(args.windows(2).any(|pair| pair == ["--profile", "fixture-jira"]));
        assert!(std::env::var_os("CRAFT_FACTORY_INTAKE_TOKEN").is_none());
        assert!(std::env::var_os("CRAFT_FACTORY_OPERATOR_TOKEN").is_none());
        assert!(std::env::var_os("CREDENTIALS_DIRECTORY").is_none());
        let site = &args[args.iter().position(|arg| arg == "--cloud-id").unwrap() + 1];
        if args[3] == "search" {
            assert!(args.iter().any(|arg| arg == "--jql=project = FIXTURE AND labels = automate"));
            assert!(args.windows(2).any(|pair| pair == ["--fields", "id,key"]));
            assert!(args.windows(2).any(|pair| pair == ["--page-size", "20"]));
            let result_site = if std::env::var_os("FACTORY_FIXTURE_JIRA_WRONG_SEARCH_SITE").is_some() {
                "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
            } else { site };
            let second_id = if std::env::var_os("FACTORY_FIXTURE_JIRA_DUPLICATE_SEARCH_ID").is_some() { "1042" } else { "1044" };
            println!(r#"{{"schema_version":"1.0","ok":true,"command":"jira.issue.search","data":{{"target":{{"provider":"jira","deployment":"cloud","tenant":"{result_site}","resource_type":"site","id":"{result_site}"}},"cached":false,"items":[{{"id":"1042","key":"FIXTURE-99"}},{{"id":"{second_id}","key":"FIXTURE-101"}}]}},"page":{{"has_more":true,"consistency":"live"}}}}"#);
            return;
        }
        assert_eq!(args[3], "get");
        let issue = &args[args.iter().position(|arg| arg == "--issue").unwrap() + 1];
        if issue == "FIXTURE-98" {
            if std::env::var_os("FACTORY_FIXTURE_JIRA_WATCH_OUTPUT_LIMIT").is_some() {
                use std::io::Write;
                let _ = std::io::stdout().write_all(&vec![b'x'; 70_000]);
                std::thread::sleep(std::time::Duration::from_secs(30));
                return;
            }
            if std::env::var_os("FACTORY_FIXTURE_JIRA_WATCH_TIMEOUT").is_some() {
                std::thread::sleep(std::time::Duration::from_secs(30));
                return;
            }
        }
        let time = std::env::var("FACTORY_FIXTURE_JIRA_OBSERVED").unwrap();
        let updated = std::env::var("FACTORY_FIXTURE_JIRA_UPDATED").unwrap();
        let assignee = if std::env::var_os("FACTORY_FIXTURE_JIRA_REASSIGNED").is_some() {
            "different-teammate"
        } else {
            "relay-human"
        };
        let assignee_field = if std::env::var_os("FACTORY_FIXTURE_JIRA_MISSING_ASSIGNEE").is_some() {
            String::new()
        } else if std::env::var_os("FACTORY_FIXTURE_JIRA_UNASSIGNED").is_some() {
            r#", "assignee":null"#.to_owned()
        } else {
            format!(r#", "assignee":{{"account_id":"{assignee}"}}"#)
        };
        let description = if std::env::var_os("FACTORY_FIXTURE_JIRA_CHANGED").is_some() {
            "Revised Jira description"
        } else {
            "Original Jira description"
        };
        let id = if std::env::var_os("FACTORY_FIXTURE_JIRA_WRONG_ID").is_some() {
            "1043"
        } else if issue == "FIXTURE-110" {
            "1045"
        } else if issue == "FIXTURE-111" {
            "1046"
        } else {
            "1042"
        };
        println!(r#"{{"schema_version":"1.0","ok":true,"command":"jira.issue.get","data":{{"target":{{"provider":"jira","deployment":"cloud","tenant":"{site}","resource_type":"site","id":"{site}"}},"requested_issue":"{issue}","identity_resolution":"matching_key","observed_at":{time},"issue":{{"id":"{id}","key":"{issue}","summary":"Standing test","description":"{description}","status":{{"id":"10001"}}{assignee_field},"labels":["automate"],"updated":"{updated}"}}}}}}"#);
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("bb") {
        assert!(std::env::vars_os().all(|(name, _)| !name.to_string_lossy()
            .to_ascii_uppercase().starts_with("CRAFT_FACTORY_")
            && !name.eq_ignore_ascii_case("CREDENTIALS_DIRECTORY")));
        let args: Vec<_> = std::env::args().collect();
        let sha = std::env::var("FACTORY_FIXTURE_SHA").unwrap();
        assert!(args.windows(2).any(|pair| pair == ["--repo", "service"]));
        match (&*args[2], &*args[3]) {
            ("pipeline", "query") => {
                assert!(args.windows(2).any(|pair| pair == ["--branch", "main"]));
                let history = if std::env::var_os("FACTORY_FIXTURE_NEWER").is_some() {
                    r#"[{"build_number":16},{"build_number":14}]"#
                } else {
                    r#"[{"build_number":14}]"#
                };
                println!("{history}");
            }
            ("pipeline", "get") if args.windows(2).any(|pair| pair == ["--pipeline", "14"]) => {
                let result = if std::env::var_os("FACTORY_FIXTURE_RECOVERED").is_some() { "SUCCESSFUL" } else { "FAILED" };
                let completed = if std::env::var_os("FACTORY_FIXTURE_RERUN").is_some() { "2026-11-21T03:45:00Z" } else { "2026-11-21T03:30:00Z" };
                let completed = std::env::var("FACTORY_FIXTURE_COMPLETED").unwrap_or_else(|_| completed.into());
                let sha = std::env::var("FACTORY_FIXTURE_PINNED_FAILURE").unwrap_or(sha);
                println!(r#"{{"build_number":14,"uuid":"{{cafe0000-0000-0000-0000-000000000042}}","repository":{{"full_name":"team/service"}},"target":{{"type":"pipeline_ref_target","ref_type":"branch","ref_name":"main","commit":{{"hash":"{sha}"}},"selector":{{"type":"default"}}}},"state":{{"name":"COMPLETED","result":{{"name":"{result}"}}}},"steps":[{{"uuid":"{{babe0000-0000-0000-0000-000000000042}}","name":"Verify candidate","completed_on":"{completed}","state":{{"name":"COMPLETED","result":{{"name":"{result}"}}}}}}]}}"#);
            }
            ("pipeline", "get") if args.windows(2).any(|pair| pair == ["--pipeline", "16"]) => {
                let selector = if std::env::var_os("FACTORY_FIXTURE_UNRELATED").is_some() { "custom" } else { "default" };
                let ref_type = if std::env::var_os("FACTORY_FIXTURE_MISSING_PIPELINE_REF").is_some() { "null" } else { "\"branch\"" };
                let selector = if std::env::var_os("FACTORY_FIXTURE_MISSING_PIPELINE_SELECTOR").is_some() {
                    "null".to_owned()
                } else {
                    format!(r#"{{"type":"{selector}","provider_extra":true}}"#)
                };
                let sha = std::env::var("FACTORY_FIXTURE_PINNED_FAILURE").unwrap_or(sha);
                println!(r#"{{"build_number":16,"repository":{{"full_name":"team/service"}},"target":{{"type":"pipeline_ref_target","ref_type":{ref_type},"ref_name":"main","commit":{{"hash":"{sha}"}},"selector":{selector}}},"state":{{"name":"IN_PROGRESS"}}}}"#);
            }
            ("pipeline", "get") => {
                assert!(args.windows(2).any(|pair| pair == ["--pipeline", "15"]));
                let base = std::env::var("FACTORY_FIXTURE_BRANCH").unwrap();
                let id = std::env::var("FACTORY_FIXTURE_PULL_NUMBER").unwrap_or_else(|_| "42".into());
                let result = if std::env::var_os("FACTORY_FIXTURE_FAILED_CHECK").is_some() { "FAILED" } else { "SUCCESSFUL" };
                println!(r#"{{"build_number":15,"uuid":"{{cafe0000-0000-0000-0000-000000000043}}","repository":{{"full_name":"team/service"}},"target":{{"commit":{{"hash":"{sha}"}},"pullrequest":{{"id":{id}}},"selector":{{"type":"pull-requests"}},"destination_commit":{{"hash":"{base}"}}}},"state":{{"name":"COMPLETED","result":{{"name":"SUCCESSFUL"}}}},"steps":[{{"uuid":"{{babe0000-0000-0000-0000-000000000043}}","name":"Verify candidate","state":{{"name":"COMPLETED","result":{{"name":"{result}"}}}}}}]}}"#);
            }
            ("pull-requests", "get") => {
                let id = args[4].parse::<u64>().unwrap();
                assert!(matches!(id, 42 | 101 | 156));
                let base = std::env::var("FACTORY_FIXTURE_BRANCH").unwrap_or_else(|_| sha.clone());
                let source = std::env::var("FACTORY_FIXTURE_SOURCE_BRANCH").unwrap_or_else(|_| {
                    if id == 101 { "craft-factory/launcher-one" } else { "feature" }.into()
                });
                let head = std::env::var("FACTORY_FIXTURE_DRAFT_HEAD").unwrap_or(sha);
                println!(r#"{{"id":{id},"state":"OPEN","draft":true,"links":{{"html":{{"href":"https://bitbucket.org/team/service/pull-requests/{id}"}}}},"source":{{"repository":{{"full_name":"team/service"}},"branch":{{"name":"{source}"}},"commit":{{"hash":"{head}"}}}},"destination":{{"branch":{{"name":"main"}},"repository":{{"full_name":"team/service"}},"commit":{{"hash":"{base}"}}}}}}"#);
            }
            ("branch", "query") => {
                let name = args[4].strip_prefix("--query=").unwrap();
                let head = if name == "main" { std::env::var("FACTORY_FIXTURE_BRANCH").unwrap_or(sha) } else { sha };
                println!(r#"[{{"name":"{name}","target":{{"hash":"{head}"}}}}]"#);
            }
            _ => panic!("unexpected TWG command"),
        }
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("bitbucket") {
        let args: Vec<_> = std::env::args().collect();
        let sha = std::env::var("FACTORY_FIXTURE_SHA").unwrap();
        let uuid = "cafe0000-0000-0000-0000-000000000042";
        let command = format!("bitbucket.{}.{}", args[2], args[3]);
        assert!(args.windows(2).any(|pair| pair == ["--profile", "saved-profile"]));
        let target = r#""target":{"provider":"bitbucket","tenant":"team"}"#;
        let (item, page) = match command.as_str() {
            "bitbucket.pullrequest.get" => (r#""item":{"id":42,"state":"OPEN","source_repository":"team/service","source_branch":"feature","destination_branch":"main"}"#.to_owned(), ""),
            "bitbucket.pipeline.get" => (format!(r#""item":{{"id":"{uuid}","build_number":14,"commit":"{sha}","state":"COMPLETED","result":"SUCCESSFUL"}}"#), ""),
            "bitbucket.step.list" => (
                r#""items":[{"id":"step-id","name":"Verify candidate","state":"COMPLETED","result":"SUCCESSFUL"}],"cached":false"#.to_owned(),
                if std::env::var_os("FACTORY_FIXTURE_TRUNCATED").is_some() {
                    r#","page":{"has_more":true,"truncated":true,"consistency":"live"}"#
                } else {
                    r#","page":{"has_more":false,"truncated":false,"consistency":"live"}"#
                }
            ),
            _ => panic!("unexpected Craft command")
        };
        println!(r#"{{"schema_version":"1.0","ok":true,"command":"{command}","data":{{{target},{item}}}{page}}}"#);
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("api") {
        let args: Vec<_> = std::env::args().collect();
        if let Some(issue) = std::env::var_os("FACTORY_FIXTURE_GITHUB_ISSUE_JSON") {
            use std::io::Write;
            assert_eq!(&args[1..], ["api", "--hostname", "github.com", "--method", "GET", "repos/team/service/issues/42"]);
            assert!(std::env::var_os("CRAFT_FACTORY_OPERATOR_TOKEN").is_none());
            assert!(std::env::var_os("CREDENTIALS_DIRECTORY").is_none());
            std::io::stdout().write_all(&std::fs::read(issue).unwrap()).unwrap();
            return;
        }
        let sha = std::env::var("FACTORY_FIXTURE_SHA").unwrap();
        let graphql = args.iter().any(|arg| arg == "graphql");
        assert!(args.iter().any(|arg| arg == if graphql { "POST" } else { "GET" }));
        if std::env::var_os("FACTORY_FIXTURE_REQUIRE_CI_ISOLATION").is_some() {
            assert!(std::env::var_os("CRAFT_FACTORY_OPERATOR_TOKEN").is_none());
            assert!(std::env::var_os("CRAFT_FACTORY_RUNNER_TOKEN").is_none());
        }
        if graphql && args.iter().any(|arg| arg.starts_with("query=mutation CraftEnqueue(")) {
            let field = |name: &str| args.iter().find_map(|arg| arg.strip_prefix(name)).unwrap();
            let intent = field("intent=");
            let pull = field("pull=");
            let head = field("head=");
            assert_eq!(head, sha);
            assert!(args.iter().any(|arg| arg.contains("expectedHeadOid:$head") && arg.contains("jump:false")));
            assert_eq!(std::env::var("GH_TOKEN").unwrap(), "fixture-github-writer-token-not-a-real-secret");
            assert!(std::env::var_os("CRAFT_FACTORY_PUBLISHER_TOKEN").is_none());
            assert!(std::env::var_os("CRAFT_FACTORY_OPERATOR_TOKEN").is_none());
            let marker = std::env::var_os("FACTORY_FIXTURE_ENQUEUE_SENT").unwrap();
            std::fs::File::create_new(marker).expect("exactly one write for the approval");
            if std::env::var_os("FACTORY_FIXTURE_ENQUEUE_LOSE_ACK").is_some() { std::process::exit(9); }
            println!(r#"{{"data":{{"enqueuePullRequest":{{"clientMutationId":"{intent}","mergeQueueEntry":{{"id":"MQE_fixture","pullRequest":{{"id":"{pull}","headRefOid":"{head}"}}}}}}}}}}"#);
        } else if graphql {
            assert!(args.iter().any(|arg| arg.starts_with("query=query CraftMergeQueueReadiness(")));
            assert!(args.iter().any(|arg| arg == "owner=team"));
            assert!(args.iter().any(|arg| arg == "name=service"));
            let id = args.iter().find_map(|arg| arg.strip_prefix("number=")).unwrap().parse::<u64>().unwrap();
            let base = std::env::var("FACTORY_FIXTURE_BASE").unwrap();
            let mode = std::env::var("FACTORY_FIXTURE_QUEUE_PR").unwrap_or_default();
            let ready = mode != "disabled";
            let mut enqueued = mode == "queued";
            if let Some(marker) = std::env::var_os("FACTORY_FIXTURE_QUEUE_READ_MARKER") {
                let marker = std::path::Path::new(&marker);
                enqueued = marker.exists();
                std::fs::write(marker, "first GraphQL observation").unwrap();
            }
            let entry = if enqueued { r#"{"id":"MQE_fixture"}"# } else { "null" };
            let auto_merge = if mode == "auto_merge" { r#"{"enabledAt":"2026-11-21T03:00:00Z"}"# } else { "null" };
            let head = if mode == "moved" { &base } else { &sha };
            let stack = if mode == "stacked" { r#"{"id":"STACK_fixture"}"# } else { "null" };
            let state = if mode == "merged" { "MERGED" } else { "OPEN" };
            let branch = if mode == "retargeted" { "other" } else { "main" };
            println!(r#"{{"data":{{"repository":{{"nameWithOwner":"team/service","pullRequest":{{"id":"PR_fixture_{id}","number":{id},"url":"https://github.com/team/service/pull/{id}","state":"{state}","isDraft":false,"isCrossRepository":false,"baseRefName":"{branch}","baseRefOid":"{base}","headRefOid":"{head}","isMergeQueueEnabled":{ready},"isInMergeQueue":{enqueued},"mergeQueueEntry":{entry},"autoMergeRequest":{auto_merge},"stack":{stack}}}}}}}}}"#);
        } else if args.iter().any(|arg| arg == "repos/team/service/branches/main/protection") {
            let admins = std::env::var_os("FACTORY_FIXTURE_PROTECTION_DRIFT").is_none();
            println!(r#"{{"required_status_checks":{{"strict":true,"contexts":["Checks"],"checks":[{{"context":"Checks","app_id":42}}]}},"required_pull_request_reviews":{{"dismiss_stale_reviews":true,"required_approving_review_count":1}},"enforce_admins":{{"enabled":{admins}}},"allow_force_pushes":{{"enabled":false}},"allow_deletions":{{"enabled":false}}}}"#);
        } else if args.iter().any(|arg| arg == "repos/team/service/rules/branches/main") {
            assert!(args.iter().any(|arg| arg == "per_page=100"));
            let mode = std::env::var("FACTORY_FIXTURE_ACTIVE_RULES").unwrap_or_default();
            let repository_rule = r#"{"type":"deletion","ruleset_source_type":"Repository","ruleset_source":"team/service","ruleset_id":56}"#;
            let org_rule = r#"{"type":"required_status_checks","ruleset_source_type":"Organization","ruleset_source":"team","ruleset_id":78,"parameters":{"strict_required_status_checks_policy":true,"required_status_checks":[{"context":"Checks","integration_id":42}]}}"#;
            let pr_rule = r#"{"type":"pull_request","ruleset_source_type":"Organization","ruleset_source":"team","ruleset_id":78,"parameters":{"dismiss_stale_reviews_on_push":true,"required_approving_review_count":1,"require_code_owner_review":false,"require_last_push_approval":false,"required_review_thread_resolution":false,"allowed_merge_methods":["squash"]}}"#;
            let queue = r#"{"type":"merge_queue","ruleset_source_type":"Repository","ruleset_source":"team/service","ruleset_id":56,"parameters":{"merge_method":"SQUASH","grouping_strategy":"ALLGREEN","check_response_timeout_minutes":20,"max_entries_to_build":5,"min_entries_to_merge":1,"max_entries_to_merge":3,"min_entries_to_merge_wait_minutes":0}}"#;
            match mode.as_str() {
                "supported" => println!("[{repository_rule},{org_rule},{pr_rule}]"),
                "reordered" => println!("[{pr_rule},{org_rule},{repository_rule}]"),
                "wrong_app" => println!("[{}]", org_rule.replace("42", "43")),
                "merge_queue" => println!(r#"[{{"type":"merge_queue","ruleset_source_type":"Organization","ruleset_source":"team","ruleset_id":78}}]"#),
                "queue" => println!("[{repository_rule},{org_rule},{pr_rule},{queue}]"),
                "queue_reordered" => println!("[{queue},{org_rule},{repository_rule},{pr_rule}]"),
                "queue_headgreen" => println!("[{repository_rule},{org_rule},{}]", queue.replace("ALLGREEN", "HEADGREEN")),
                "queue_duplicate" => println!("[{queue},{queue}]"),
                "limit" => println!("[{}]", vec![repository_rule; 100].join(",")),
                _ => println!("[]"),
            }
        } else if let Some(ruleset) = args.iter().find_map(|arg| arg.strip_prefix("repos/team/service/rulesets/")) {
            assert!(args.iter().any(|arg| arg == "includes_parents=true"));
            let (kind, source) = match ruleset {
                "56" => ("Repository", "team/service"),
                "78" => ("Organization", "team"),
                _ => panic!("unrecognized ruleset fixture"),
            };
            let bypass = match std::env::var("FACTORY_FIXTURE_QUEUE_BYPASS").as_deref() {
                Ok("present") if ruleset == "78" => r#","bypass_actors":[{"actor_id":42,"actor_type":"Team","bypass_mode":"always"}]"#,
                Ok("hidden") if ruleset == "78" => "",
                _ => r#","bypass_actors":[]"#,
            };
            println!(r#"{{"id":{ruleset},"target":"branch","source_type":"{kind}","source":"{source}","enforcement":"active","conditions":{{"ref_name":{{"include":["refs/heads/main"],"exclude":[]}}}}{bypass}}}"#);
        } else if let Some(compare) = args.iter().find_map(|arg| arg.strip_prefix("repos/team/service/compare/")) {
            assert!(args.iter().any(|arg| arg == "per_page=1"));
            let (ancestor, _) = compare.split_once("...").unwrap();
            let diverged = std::env::var_os("FACTORY_FIXTURE_MERGE_DIVERGED").is_some()
                || (ancestor == "c".repeat(40)
                    && std::env::var_os("FACTORY_FIXTURE_BRANCH_REWRITTEN").is_some());
            let status = if diverged { "diverged" } else { "ahead" };
            let behind = usize::from(diverged);
            println!(r#"{{"base_commit":{{"sha":"{ancestor}"}},"merge_base_commit":{{"sha":"{ancestor}"}},"status":"{status}","ahead_by":1,"behind_by":{behind}}}"#);
        } else if args.iter().any(|arg| arg == "repos/team/service/branches/main") {
            let base = std::env::var("FACTORY_FIXTURE_BASE").unwrap();
            let mut branch = std::env::var("FACTORY_FIXTURE_BRANCH").unwrap_or(base);
            if let Some(path) = std::env::var_os("FACTORY_FIXTURE_BRANCH_TIP_MARKER") {
                if std::path::Path::new(&path).is_file() {
                    branch = std::env::var("FACTORY_FIXTURE_BRANCH_AFTER").unwrap();
                } else {
                    std::fs::write(path, "first branch observation").unwrap();
                }
            }
            println!(r#"{{"name":"main","protected":true,"commit":{{"sha":"{branch}"}}}}"#);
        } else if args.iter().any(|arg| arg == "repos/team/service/actions/workflows/9/runs") {
            if let Some(path) = std::env::var_os("FACTORY_FIXTURE_REPLACE_CI_POLICY") {
                std::fs::write(path, "policy changed after the cycle read it").unwrap();
            }
            let base = std::env::var("FACTORY_FIXTURE_BASE").unwrap();
            for field in ["branch=main".to_owned(), format!("head_sha={base}"), "event=push".into(), "per_page=30".into()] {
                assert!(args.contains(&field));
            }
            let attempt = if std::env::var_os("FACTORY_FIXTURE_RERUN").is_some() { 3 } else { 2 };
            let row = |id, attempt, number, conclusion| format!(r#"{{"id":{id},"run_attempt":{attempt},"run_number":{number},"workflow_id":9,"event":"push","status":"completed","conclusion":"{conclusion}","head_sha":"{base}","head_branch":"main","repository":{{"full_name":"team/service"}},"head_repository":{{"full_name":"team/service"}}}}"#);
            let mut rows = vec![row(73, attempt, 4, "failure")];
            if std::env::var_os("FACTORY_FIXTURE_NEWER").is_some() {
                rows.push(row(74, 1, 5, "success"));
            }
            let count = rows.len() + usize::from(std::env::var_os("FACTORY_FIXTURE_TRUNCATE_WORKFLOWS").is_some());
            println!(r#"{{"total_count":{count},"workflow_runs":[{}]}}"#, rows.join(","));
        } else if args.iter().any(|arg| arg == "repos/team/service/actions/runs/73") {
            let base = std::env::var("FACTORY_FIXTURE_BASE").unwrap();
            let attempt = if std::env::var_os("FACTORY_FIXTURE_RERUN").is_some() { 3 } else { 2 };
            println!(r#"{{"id":73,"run_attempt":{attempt},"run_number":4,"workflow_id":9,"event":"push","status":"completed","conclusion":"failure","head_sha":"{base}","head_branch":"main","repository":{{"full_name":"team/service"}},"head_repository":{{"full_name":"team/service"}}}}"#);
        } else if let Some(attempt) = args.iter().find_map(|arg| match arg.as_str() {
            "repos/team/service/actions/runs/73/attempts/2/jobs" => Some(2),
            "repos/team/service/actions/runs/73/attempts/3/jobs" => Some(3),
            _ => None,
        }) {
            assert!(args.iter().any(|arg| arg == "per_page=100"));
            let base = std::env::var("FACTORY_FIXTURE_BASE").unwrap();
            let count = if std::env::var_os("FACTORY_FIXTURE_TRUNCATE_JOBS").is_some() { 2 } else { 1 };
            let id = if attempt == 3 { 22 } else { 21 };
            let conclusion = if std::env::var_os("FACTORY_FIXTURE_OTHER_JOB").is_some() { "success" } else { "failure" };
            println!(r#"{{"total_count":{count},"jobs":[{{"id":{id},"run_id":73,"name":"Checks","head_sha":"{base}","status":"completed","conclusion":"{conclusion}","completed_at":"2026-11-21T03:30:00Z"}}]}}"#);
        } else if args.iter().any(|arg| arg == "user") {
            assert!(std::env::var_os("CRAFT_FACTORY_REVIEWER_TOKEN").is_none());
            assert!(std::env::var_os("CRAFT_FACTORY_REVIEWER_TOKEN_FILE").is_none());
            let id = if std::env::var_os("FACTORY_FIXTURE_WRONG_REVIEW_LOGIN").is_some() { 99 } else { 88 };
            println!(r#"{{"id":{id}}}"#);
        } else if args.iter().any(|arg| arg == "repos/team/service/pulls/42/reviews/601" || arg == "repos/team/service/pulls/42/reviews") {
            let reviewer = if std::env::var_os("FACTORY_FIXTURE_REVIEW_SELF").is_some() { 77 } else { 88 };
            let reviewed = std::env::var("FACTORY_FIXTURE_REVIEW_HEAD").unwrap_or(sha);
            let state = if std::env::var_os("FACTORY_FIXTURE_REVIEW_DISMISSED").is_some() { "DISMISSED" } else { "CHANGES_REQUESTED" };
            let body = if std::env::var_os("FACTORY_FIXTURE_SHARED_REVIEW_BODY").is_some() {
                r#"Preserve the reviewed change\nand say repaired"#
            } else {
                "unreviewed forge text fixture"
            };
            let row = |id, state| format!(r#"{{"id":{id},"user":{{"id":{reviewer},"type":"User"}},"state":"{state}","commit_id":"{reviewed}","submitted_at":"2026-11-21T03:30:00Z","pull_request_url":"https://api.github.com/repos/team/service/pulls/42","body":"{body}"}}"#);
            let selected = row(601, state);
            if args.iter().any(|arg| arg.ends_with("/601")) {
                println!("{selected}");
            } else {
                assert!(args.iter().any(|arg| arg == "per_page=100"));
                assert!(args.iter().any(|arg| arg == "page=1"));
                if std::env::var_os("FACTORY_FIXTURE_MERGED").is_some() {
                    let actor = if std::env::var_os("FACTORY_FIXTURE_APPROVAL_SELF").is_some() { 77 } else { 88 };
                    let approved_head = if std::env::var_os("FACTORY_FIXTURE_APPROVAL_STALE").is_some() {
                        std::env::var("FACTORY_FIXTURE_BASE").unwrap()
                    } else { reviewed.clone() };
                    let approved_at = if std::env::var_os("FACTORY_FIXTURE_APPROVAL_LATE").is_some() { "04:30" } else { "03:50" };
                    let approval_state = if std::env::var_os("FACTORY_FIXTURE_APPROVAL_DISMISSED").is_some() { "DISMISSED" } else { "APPROVED" };
                    let approval = format!(r#"{{"id":603,"user":{{"id":{actor},"type":"User"}},"state":"{approval_state}","commit_id":"{approved_head}","submitted_at":"2026-11-21T{approved_at}:00Z","pull_request_url":"https://api.github.com/repos/team/service/pulls/42"}}"#);
                    let follow_up = if std::env::var_os("FACTORY_FIXTURE_APPROVAL_OBJECTED").is_some() {
                        format!(r#",{{"id":604,"user":{{"id":88,"type":"User"}},"state":"CHANGES_REQUESTED","commit_id":"{reviewed}","submitted_at":"2026-11-21T03:55:00Z","pull_request_url":"https://api.github.com/repos/team/service/pulls/42"}}"#)
                    } else { String::new() };
                    println!("[{approval}{follow_up}]");
                } else if std::env::var_os("FACTORY_FIXTURE_GITHUB_SHADOW").is_some() {
                    let approval = if std::env::var_os("FACTORY_FIXTURE_APPROVAL_STALE").is_some() { std::env::var("FACTORY_FIXTURE_BASE").unwrap() } else { reviewed.clone() };
                    println!(r#"[{{"id":603,"user":{{"id":88,"type":"User"}},"state":"APPROVED","commit_id":"{approval}","submitted_at":"2025-11-21T03:50:00Z","pull_request_url":"https://api.github.com/repos/team/service/pulls/42"}}]"#);
                } else {
                    let follow_up = if std::env::var_os("FACTORY_FIXTURE_REVIEW_SUPERSEDED").is_some() { "APPROVED" } else { "COMMENTED" };
                    println!("[{selected},{}]", row(602, follow_up));
                }
            }
        } else if args.iter().any(|arg| arg.ends_with("/check-runs")) {
            assert!(args.iter().any(|arg| arg == "app_id=42"));
            if let Some(gate) = std::env::var_os("FACTORY_FIXTURE_CHECK_GATE") {
                let gate = std::path::PathBuf::from(gate);
                std::fs::write(gate.join("waiting"), "").unwrap();
                let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while !gate.join("release").exists() {
                    assert!(std::time::Instant::now() < until, "timed out waiting for test");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            let conclusion = if std::env::var_os("FACTORY_FIXTURE_FAILED_CHECK").is_some() { "failure" } else { "success" };
            println!(r#"{{"total_count":1,"check_runs":[{{"id":7,"name":"Checks","app":{{"id":42}},"head_sha":"{sha}","status":"completed","conclusion":"{conclusion}"}}]}}"#);
        } else {
            let id = std::env::var("FACTORY_FIXTURE_PULL_NUMBER").ok()
                .map(|value| value.parse::<u64>().unwrap()).unwrap_or(42);
            assert!(args.iter().any(|arg| arg == &format!("repos/team/service/pulls/{id}")));
            if std::env::var_os("FACTORY_FIXTURE_REQUIRE_VERIFIER_ISOLATION").is_some() {
                assert!(std::env::var_os("CRAFT_FACTORY_VERIFIER_TOKEN").is_none());
                assert!(std::env::var_os("CRAFT_FACTORY_VERIFIER_TOKEN_FILE").is_none());
                assert!(std::env::var_os("CRAFT_FACTORY_OPERATOR_TOKEN").is_none());
            }
            if std::env::var_os("FACTORY_FIXTURE_HANG_MERGE_READ").is_some() {
                std::thread::sleep(std::time::Duration::from_secs(10));
            }
            let base = std::env::var("FACTORY_FIXTURE_BASE").unwrap();
            if std::env::var_os("FACTORY_FIXTURE_MERGED").is_some() {
                let timestamp = std::env::var("FACTORY_FIXTURE_MERGE_TIME").unwrap_or_else(|_| "2026-11-21T04:00:00Z".into());
                let result = "c".repeat(40);
                let repo_id = if std::env::var_os("FACTORY_FIXTURE_WRONG_GITHUB_REPO_ID").is_some() { 101 } else { 100 };
                println!(r#"{{"number":{id},"html_url":"https://github.com/team/service/pull/{id}","state":"closed","user":{{"id":77}},"created_at":"2024-12-31T23:00:00Z","merged":true,"merged_at":"{timestamp}","merged_by":{{"id":88}},"merge_commit_sha":"{result}","base":{{"ref":"main","sha":"{result}","repo":{{"id":{repo_id},"full_name":"team/service"}}}},"head":{{"sha":"{sha}","repo":{{"id":100,"full_name":"team/service"}}}}}}"#);
            } else {
                let mergeable = if std::env::var_os("FACTORY_FIXTURE_MERGE_UNKNOWN").is_some() { "null" } else { "true" };
                let source = std::env::var("FACTORY_FIXTURE_SOURCE_BRANCH").unwrap_or_default();
                let draft = !source.is_empty();
                println!(r#"{{"number":{id},"node_id":"PR_fixture_{id}","html_url":"https://github.com/team/service/pull/{id}","state":"open","merged":false,"draft":{draft},"mergeable":{mergeable},"mergeable_state":"clean","user":{{"id":77}},"base":{{"ref":"main","sha":"{base}","repo":{{"full_name":"team/service"}}}},"head":{{"ref":"{source}","sha":"{sha}","repo":{{"full_name":"team/service"}}}}}}"#);
            }
        }
        return;
    }
    let mut prompt = String::new();
    std::io::stdin().read_to_string(&mut prompt).unwrap();
    for (intent, model, effort) in [
        ("Routine profile fixture", "fixture-mini", "low"),
        ("Deep profile fixture", "fixture-reasoning", "high"),
    ] {
        if prompt.contains(intent) {
            let args: Vec<_> = std::env::args().collect();
            let config = format!("model_reasoning_effort=\"{effort}\"");
            let exec = args.iter().position(|arg| arg == "exec").unwrap();
            let input = args.iter().position(|arg| arg == "-").unwrap();
            assert!(args[exec..input].windows(2).any(|pair| pair == ["--model", model]));
            assert!(args[exec..input].windows(2).any(|pair| pair == ["-c", config.as_str()]));
        }
    }
    if prompt.contains("team:fixture-release") {
        assert!(prompt.contains("Reviewed cross-repo skill"));
        assert!(!prompt.contains("Pinned skill for fixture"));
        assert!(!prompt.contains("Unapproved personal skill"));
    } else if prompt.contains("team:bitbucket-merge-fixture") {
        assert!(prompt.contains("Reviewed Bitbucket merge skill"));
        assert!(!prompt.contains("Pinned skill for fixture"));
    } else {
        assert!(prompt.contains("Pinned skill for fixture"));
    }
    if prompt.contains("project:fixture-routing") {
        assert!(prompt.contains("Approved replacement routing decision"));
        assert!(prompt.contains("Approved runbook excerpt"));
        assert!(!prompt.contains("Outdated routing decision"));
    }
    assert!(std::env::var_os("FACTORY_TEST_SECRET").is_none());
    assert!(std::env::var_os("CRAFT_FACTORY_RUNNER_TOKEN_FILE").is_none());
    assert!(std::env::var_os("CREDENTIALS_DIRECTORY").is_none());
    assert!(std::env::var_os("CRAFT_FACTORY_RUNNER_TOKEN").is_none());
    if prompt.contains("Wait for timeout")
        || std::env::var_os("CODEX_HOME")
            .map(std::path::PathBuf::from)
            .is_some_and(|home| home.join("fixture-wait").exists())
    {
        std::thread::sleep(std::time::Duration::from_secs(30));
        return;
    }
    if prompt.contains("Wait for planned runner stop") {
        let home = std::path::PathBuf::from(std::env::var_os("CODEX_HOME").unwrap());
        let release = home.join("fixture-drain-release");
        std::fs::write(home.join("fixture-drain-started"), "").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !release.exists() {
            assert!(std::time::Instant::now() < deadline, "fixture runner did not release worker");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    if prompt.contains("Second round on the same draft") {
        assert!(prompt.contains("Handle the empty value"));
        assert!(prompt.contains("fixture:first-review"));
        assert!(prompt.contains("fixture:second-review-101"));
    }
    if prompt.contains("Preserve the reviewed change and say repaired") {
        assert_eq!(
            std::fs::read_to_string("src/hello").unwrap(),
            "changed by fixture agent"
        );
        std::fs::write("src/hello", "repaired from reviewed revision").unwrap();
    } else {
        std::fs::write("src/hello", "changed by fixture agent").unwrap();
    }
    assert!(prompt.contains("Leave your edits uncommitted"));
    let requested = prompt.contains("one unapproved knowledge_proposal");
    let knowledge = if prompt.contains("Return an invalid knowledge proposal") {
        r#",\"knowledge_proposal\":{\"location\":\"project:missing-fields\"}"#
    } else if prompt.contains("Return a knowledge proposal") {
        r##",\"knowledge_proposal\":{\"kind\":\"skill\",\"location\":\"team:worker-fixture-skill\",\"body\":\"# Compare fixture timestamps\",\"evidence\":\"src/hello\"}"##
    } else {
        ""
    };
    if requested {
        let args: Vec<_> = std::env::args().collect();
        let schema = args.windows(2).find(|pair| pair[0] == "--output-schema").unwrap();
        let schema = std::fs::read_to_string(&schema[1]).unwrap();
        assert!(schema.contains(r#""knowledge_proposal""#));
    }
    println!(r#"{{"type":"item.completed","item":{{"type":"agent_message","text":"{{\"outcome\":\"candidate\",\"summary\":\"The scoped edit is ready.\"{knowledge}}}"}}}}"#);
    if prompt.contains("Return an invalid final result") {
        println!(r#"{{"type":"item.completed","item":{{"type":"agent_message","text":"Unexpected final result"}}}}"#);
    }
    println!(r#"{{"type":"turn.completed","usage":{{"input_tokens":21,"output_tokens":9}}}}"#);
}
