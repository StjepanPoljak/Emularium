# Running Claude Code safely, with or without Docker

Written 2026-10-11. Applies to all projects, not just this one; move it
somewhere shared if you like. Setting names are Claude Code settings.json keys;
run `/permissions` and `/sandbox` in a session to check what's active.

## Four layers, strongest first

| Layer | What it stops | Cost |
|---|---|---|
| **Docker container** | Everything outside the mounted project dir: your home, keys, other projects, host packages | Needs an image per environment; tools must be installed in the image |
| **Claude Code sandbox** (`sandbox.enabled`) | Bash commands writing outside the project, reading denied paths, and reaching non-allowlisted network hosts. Enforced by the OS (bubblewrap on Linux) | Linux needs `bubblewrap` and `socat` installed. Some tools (adb, ssh) need exceptions |
| **Permission rules** (`allow` / `ask` / `deny`) | Decides which tool calls run without asking, which prompt you, which are refused | Matching is by pattern. A Bash deny rule like `Bash(rm -rf *)` is a guard rail, not a wall: the same thing can be spelled differently (`bash -c ...`, a script) |
| **Permission mode** (`defaultMode`) | `default` asks for anything not allowed; `acceptEdits` auto-accepts file edits; `auto` lets a classifier approve routine actions | Choose per session with shift+tab or `/config` |

How to use them together:
- **Docker projects:** what you do now. The container is the boundary;
  permission rules just reduce prompts.
- **Lightweight projects without Docker:** turn on the **sandbox**. It's the
  real boundary there. Permission rules alone are not enough for "I'm afraid
  it'll run some weird thing", because a deny list can't list every weird thing.

## Where settings live

| File | Scope | Commit? |
|---|---|---|
| `~/.claude/settings.json` | Every project on that machine (put the safety baseline here) | n/a |
| `<project>/.claude/settings.json` | One project: its build tools and network hosts | Yes |
| `<project>/.claude/settings.local.json` | One project, just you | No (gitignore it) |

Precedence: user, then project, then local; later files override earlier ones,
and arrays like `allow`/`deny` merge. **A deny anywhere wins over an allow.**

Note: the container and the host each have their own `~/.claude`. The host
baseline below must be installed on the host itself.

## Host baseline: `~/.claude/settings.json`

```json
{
  "permissions": {
    "defaultMode": "default",
    "deny": [
      "Bash(sudo *)",
      "Bash(su *)",
      "Bash(rm -rf *)",
      "Bash(git push *)",
      "Bash(curl * | sh*)",
      "Bash(wget * | sh*)",
      "Read(~/.ssh/**)",
      "Read(~/.gnupg/**)",
      "Read(~/.aws/**)",
      "Read(**/.env)",
      "Edit(~/.bashrc)",
      "Edit(~/.profile)",
      "Edit(~/.ssh/**)"
    ],
    "ask": [
      "Bash(git commit *)",
      "Bash(ssh *)",
      "Bash(scp *)",
      "Bash(rsync *)",
      "Bash(docker *)"
    ]
  },
  "sandbox": {
    "enabled": true,
    "autoAllowBashIfSandboxed": true,
    "allowUnsandboxedCommands": false,
    "network": {
      "allowedDomains": ["github.com", "*.githubusercontent.com"]
    }
  }
}
```

What this does:
- **`sandbox.enabled`:** Bash commands can only write inside the project
  directory, and can only reach the listed hosts.
- **`allowUnsandboxedCommands: false`:** removes Claude's ability to ask for a
  command to run outside the sandbox. This is the setting that addresses
  "weird things": nothing escapes, even with your approval in a hurry.
- **`autoAllowBashIfSandboxed`:** sandboxed commands run without prompting,
  since the sandbox already contains them. Set it to `false` if you'd rather
  approve each command anyway.
- **`deny`** keeps secrets unreadable and blocks a few obviously dangerous
  commands. **`ask`** makes anything that leaves the machine (ssh, push,
  deploys) need your OK.

Inside Docker you can leave the sandbox off (`"enabled": false` in the
container's `~/.claude/settings.json`); the container already does that job.

## Per-project files

Add only what that project needs. Arrays merge with the baseline.

### Emularium (Rust, in Docker)

`.claude/settings.json`:

```json
{
  "permissions": {
    "allow": [
      "Bash(cargo build *)", "Bash(cargo test *)", "Bash(cargo run *)",
      "Bash(cargo check *)", "Bash(cargo clippy *)",
      "Bash(gpasm *)", "Bash(gpsim *)",
      "Bash(git status *)", "Bash(git diff *)", "Bash(git log *)"
    ]
  },
  "sandbox": {
    "network": {
      "allowedDomains": ["crates.io", "index.crates.io", "static.crates.io"]
    }
  }
}
```

### Android app -> HTTP -> Raspberry Pi audio player (example, adjust)

Two parts with different risks: building the Android app (local, needs Gradle
and Maven hosts) and deploying to the Pi (leaves the machine, so ask).

```json
{
  "permissions": {
    "allow": [
      "Bash(./gradlew assembleDebug *)", "Bash(./gradlew test *)",
      "Bash(./gradlew lint *)"
    ],
    "ask": [
      "Bash(adb install *)", "Bash(adb shell *)",
      "Bash(ssh pi@*)", "Bash(scp * pi@*)", "Bash(rsync * pi@*)"
    ]
  },
  "sandbox": {
    "excludedCommands": ["adb *", "ssh *", "scp *", "rsync *"],
    "network": {
      "allowedDomains": [
        "dl.google.com", "maven.google.com", "repo.maven.apache.org",
        "plugins.gradle.org", "services.gradle.org"
      ]
    }
  }
}
```

- `excludedCommands` run outside the sandbox, because adb talks to a local
  server and ssh isn't HTTP. They still go through the `ask` prompts, so they
  aren't a loophole. (Not tested yet: confirm adb and ssh actually need this on
  your machine.)
- If the Pi side is a separate repo (a Python or Rust HTTP server), give it
  its own file. To test it locally, allow the run command plus
  `curl http://localhost:*`.

## Checklist for a new project

1. Docker or not? Docker for anything with a heavy or fussy toolchain.
   Sandbox for lightweight projects.
2. Add `.claude/settings.json` with the build, test and run commands, plus the
   package registry hosts.
3. Put anything that touches other machines (deploy, push, ssh) in `ask`.
4. Start a session and run `/permissions` and `/sandbox` to confirm the rules
   loaded.
5. Optional: run `/fewer-permission-prompts` after a few sessions. It suggests
   allow rules based on what you actually approved.
