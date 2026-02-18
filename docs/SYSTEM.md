# nanobot System Architecture

## Core System (src/identity.rs)

### Basic Functions
- CLI Interaction
- Voice UI
- File Operations
- Shell Command Execution
- Web Search & Fetch
- Multi-channel Messaging
- Background Task Management
- Easter Eggs & Omikuji

### Supported Channels
- CLI
- Voice
- Web
- LINE
- Telegram
- Discord
- WhatsApp
- Teams
- Slack

### Core Characteristics
- Name: nanobot
- Version: 2.0.0
- Personality: Curious, proactive, technically precise
- Runtime: Rust on macOS aarch64
- Memory System: Persistent storage in workspace/memory/

## Extension System (crates/)

### nanobot-core
- Core functionality implementation
- Tool system
- Memory management
- Channel integration

### nanobot-lambda
- AWS Lambda support
- Serverless execution
- Performance optimization

## Infrastructure (infra/)
- AWS Lambda configuration
- Deployment management
- Scaling settings
- Monitoring

## Documentation (docs/)
- API specifications
- Implementation guides
- Security audits
- Performance optimization
- Caching system
- Health checks

## Testing (tests/)
- Unit tests
- Integration tests
- Performance tests

## Web Interface (web/)
- Frontend implementation
- API endpoints
- User interface