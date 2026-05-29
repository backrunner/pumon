# Service Support

The current service implementation generates a user-level startup definition that runs:

```text
procwatch daemon run <config>
```

Platform output:

- macOS: `~/Library/LaunchAgents/top.backrunner.procwatch.plist`
- Linux: `~/.config/systemd/user/procwatch.service`
- Windows: a command file under `PROCWATCH_HOME/service`

On macOS the generated LaunchAgent writes stdout and stderr to `PROCWATCH_HOME/daemon/service.out.log` and `PROCWATCH_HOME/daemon/service.err.log`.

`procwatch service start` and `procwatch service stop` call `launchctl` on macOS and `systemctl --user` on Linux. `procwatch service status` also reports backend-specific state such as loaded, active, and enabled where the platform supports it. Windows service registration is not implemented yet.
