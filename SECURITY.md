# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.x.x   | :white_check_mark: |

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

Instead, please report them via:
- **GitHub Security Advisories**: [Report a vulnerability](https://github.com/yukihamada/nanobot/security/advisories/new)
- **Email**: security@chatweb.ai (PGP key available on request)

Please include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

We will acknowledge your report within 48 hours and provide a detailed response within 7 days.

## Security Features

### Authentication & Authorization
- **Password hashing** - HMAC-SHA256 with configurable secret keys
- **Session tokens** - 32-byte cryptographically secure random tokens
- **Google OAuth** - PKCE flow with state validation
- **Admin keys** - Configurable via `ADMIN_SESSION_KEYS` environment variable
- **Rate limiting** - 5 login attempts/min, 3 registrations/min (DynamoDB atomic counters)

### Code Execution Sandboxing
- **Isolated sandbox** - Each session gets isolated `/tmp/sandbox/{session_id}/` directory
- **Path traversal protection** - File operations reject `..` sequences
- **Command restrictions** - Only allow safe shell commands by default
- **Timeout limits** - 10-second execution timeout for code execution

### Data Protection
- **Input validation** - 1MB body limit, 32K message limit, strict email/password constraints
- **CORS restrictions** - Whitelist: `chatweb.ai`, `api.chatweb.ai`, `localhost:3000`
- **Webhook verification** - X-Telegram-Bot-Api-Secret-Token, Facebook verify_token, Stripe signature validation
- **DynamoDB encryption** - At-rest encryption with AWS KMS
- **Audit logging** - All auth events logged with 90-day TTL

### Infrastructure
- **API Gateway** - AWS WAF integration for DDoS protection
- **Secrets management** - Environment variables, AWS Secrets Manager support
- **TLS/SSL** - All traffic encrypted with HTTPS
- **No secrets in logs** - API keys and tokens are never logged

## Known Limitations

1. **Lambda AL2023 runtime** - No Python/Node.js interpreters available → `code_execute` shell only
2. **Sandboxed execution** - File operations are restricted to session sandbox
3. **Rate limiting** - Per-user, not per-IP (Cloudflare handles IP-based rate limiting)

## Best Practices for Self-Hosting

1. **Set strong secrets**
   ```bash
   export PASSWORD_HMAC_KEY="$(openssl rand -hex 32)"
   export ADMIN_SESSION_KEYS="admin-$(openssl rand -hex 16)"
   ```

2. **Restrict CORS origins**
   - Update `ALLOWED_ORIGINS` to your domain only

3. **Use AWS Secrets Manager**
   ```bash
   aws secretsmanager create-secret --name nanobot/api-keys \
     --secret-string '{"OPENAI_API_KEY":"sk-..."}'
   ```

4. **Enable CloudWatch logs**
   - Monitor for suspicious activity
   - Set up alerts for failed auth attempts

5. **Regular updates**
   ```bash
   git pull origin main
   cargo update
   cargo build --release
   ```

## Security Updates

Security updates are released as soon as possible after a vulnerability is confirmed. Follow [@yukihamada](https://github.com/yukihamada) or watch this repo for notifications.

## Disclosure Policy

- **Private disclosure** - 90 days before public disclosure
- **CVE assignment** - We will request CVE IDs for confirmed vulnerabilities
- **Credit** - Security researchers will be credited (unless they prefer to remain anonymous)

## Bug Bounty

We currently do not have a formal bug bounty program, but we deeply appreciate security researchers' efforts. Responsible disclosures will be publicly acknowledged.

---

**Last updated:** 2026-02-17
