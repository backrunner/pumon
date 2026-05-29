# Service Support

The current service implementation generates a user-level startup definition that runs:

```text
promon daemon run <config>
```

Platform output:

- macOS: `~/Library/LaunchAgents/top.backrunner.promon.plist`
- Linux: `~/.config/systemd/user/promon.service`
- Windows: a command file under `PROMON_HOME/service`

On macOS the generated LaunchAgent writes stdout and stderr to `PROMON_HOME/daemon/service.out.log` and `PROMON_HOME/daemon/service.err.log`.

`promon service start` and `promon service stop` call `launchctl` on macOS and `systemctl --user` on Linux. `promon service status` also reports backend-specific state such as loaded, active, and enabled where the platform supports it. Windows service registration is not implemented yet.
