# Service Support

The current service implementation generates a user-level startup definition that runs:

```text
promon start --wait <config>
```

Platform output:

- macOS: `~/Library/LaunchAgents/top.backrunner.promon.plist`
- Linux: `~/.config/systemd/user/promon.service`
- Windows: a command file under `PROMON_HOME/service`

Native daemon service registration remains the next hardening step.

