# Service Support

The current service implementation generates a user-level startup definition that runs:

```text
promon daemon run <config>
```

Platform output:

- macOS: `~/Library/LaunchAgents/top.backrunner.promon.plist`
- Linux: `~/.config/systemd/user/promon.service`
- Windows: a command file under `PROMON_HOME/service`

`promon service start` and `promon service stop` call `launchctl` on macOS and `systemctl --user` on Linux. Windows native service registration remains the next hardening step.
