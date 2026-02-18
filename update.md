# Auto Update System

## Version Check
- Current Version: 2.0.0
- Update Check Interval: Daily
- Auto Update: Enabled

## Update Sources
- GitHub Repository: github.com/yukihamada/nanobot
- Release Channel: stable
- Beta Channel: beta (opt-in)

## Update Process
1. Daily version check at startup
2. Notification of available updates
3. Automatic download of updates
4. Verification of downloaded files
5. Backup of current version
6. Installation of new version
7. Validation of installation
8. Rollback on failure

## Update Commands
```bash
/version        # Show current version
/check-update   # Check for updates
/update         # Update manually
/rollback       # Rollback to previous version
```

## Auto Update Settings
```toml
[update]
auto_check = true
auto_install = true
check_interval = "daily"
channel = "stable"
backup = true
notifications = true
```

## Update Log
- Location: /Users/yuki/.nanobot/logs/update.log
- Backup Location: /Users/yuki/.nanobot/backup/
- Version History: /Users/yuki/.nanobot/versions.json