# Security policy

## Reporting a vulnerability

Please report suspected vulnerabilities privately through GitHub's Security Advisories for this repository. If private reporting is unavailable, contact the maintainers through GitHub and request a private channel before publishing exploit details. Include affected versions, reproduction steps and impact. Do not include secrets or customer data.

## Security boundaries

Herdr Organizations is a local CLI and Herdr plugin. Project state and profiles are stored in project files. Agents run with the permissions of their own harness and can execute commands available to their user account.

- `can_spawn` and known pane-to-parent checks are enforced by the CLI during node creation. They are not an ACL against an agent that can edit project files, clear caller context or run arbitrary commands.
- Permission profiles select harness CLI options. They do not replace operating system permissions, sandbox policy or the user's normal approval flow.
- `--raw-agent-arg` intentionally passes one supplied value to the selected harness. The application passes argv components directly and does not evaluate them through a shell.
- Root instructions and memory, then ancestor and target node scopes, are included in a node brief. Sibling and descendant scopes are excluded. Node scopes and memory folders must be regular directories; symbolic-link scope files are not followed.
- Legacy project and settings paths are `~/.herdr-projects/` and `~/.config/herdr-projects/`. Keep credentials and unrelated secrets out of project context because instructions and memory are sent to agent CLIs.
- Remote execution uses the user's configured SSH and Herdr machine. The local project remains authoritative for records and home reports.
- The application has no daemon, network service, database, MCP server or harness-registered tools.

Use a disposable project and repository when validating profiles, raw argv or agent permissions. Review changes to project context and `thread_agent_args` before launching agents.
