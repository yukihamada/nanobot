"""
Browser Automation Service on Modal — Playwright-based headless browser for chatweb.ai.

Deploy:  modal deploy browser_service.py
Test:    modal run browser_service.py
Logs:    modal app logs browser-service

Exposes:
  POST   /session/create            — Create a new browser session
  POST   /session/{id}/actions      — Execute action batch (navigate, click, fill, screenshot, etc.)
  POST   /session/{id}/screenshot   — Take screenshot of current page
  DELETE /session/{id}              — Close session
  GET    /health                    — Health check
"""
import modal
import uuid
import asyncio
import base64
import time

# ---------------------------------------------------------------------------
# Modal image: Playwright + Chromium
# ---------------------------------------------------------------------------
browser_image = (
    modal.Image.debian_slim(python_version="3.11")
    .apt_install("wget", "ca-certificates", "fonts-noto-cjk")
    .pip_install(
        "playwright==1.49.1",
        "fastapi[standard]",
    )
    .run_commands("python3 -m playwright install --with-deps chromium")
)

app = modal.App("browser-service", image=browser_image)

# ---------------------------------------------------------------------------
# Session management — in-memory browser contexts keyed by session_id
# ---------------------------------------------------------------------------
# Sessions are stored per-container. scaledown_window keeps container alive.
_sessions: dict[str, dict] = {}
_lock = asyncio.Lock()

SESSION_TIMEOUT_SECS = 300  # 5 min idle timeout

# ---------------------------------------------------------------------------
# Web endpoint
# ---------------------------------------------------------------------------
@app.function(
    cpu=2.0,
    memory=2048,
    timeout=600,
    min_containers=0,
    scaledown_window=600,  # Keep container alive 10 min after last request
)
@modal.concurrent(max_inputs=4)
@modal.asgi_app()
def web():
    from fastapi import FastAPI, HTTPException
    from fastapi.responses import JSONResponse
    from pydantic import BaseModel, Field
    from playwright.async_api import async_playwright

    fastapi_app = FastAPI(title="Browser Automation Service", version="1.0")

    # Lazy-initialized Playwright instance (shared across requests in same container)
    _playwright = None
    _browser = None

    async def get_browser():
        nonlocal _playwright, _browser
        if _browser is None or not _browser.is_connected():
            _playwright = await async_playwright().start()
            _browser = await _playwright.chromium.launch(
                headless=True,
                args=[
                    "--no-sandbox",
                    "--disable-dev-shm-usage",
                    "--disable-gpu",
                    "--disable-extensions",
                    "--lang=ja-JP,ja,en-US,en",
                ],
            )
        return _browser

    # ---- Request models ----

    class CreateSessionRequest(BaseModel):
        user_agent: str | None = Field(
            default=None,
            description="Custom User-Agent string",
        )
        viewport_width: int = Field(default=1280, ge=320, le=1920)
        viewport_height: int = Field(default=720, ge=240, le=1080)
        locale: str = Field(default="ja-JP")

    class Action(BaseModel):
        type: str = Field(
            description="Action type: navigate, click, fill, fill_credentials, "
            "select, screenshot, wait_navigation, wait_selector, scroll, "
            "evaluate, go_back, go_forward"
        )
        url: str | None = None
        selector: str | None = None
        value: str | None = None
        timeout_ms: int = Field(default=10000, ge=1000, le=30000)
        # For fill_credentials
        service_name: str | None = None
        username_selector: str | None = None
        password_selector: str | None = None
        encrypted_username: str | None = None
        encrypted_password: str | None = None
        # For evaluate
        expression: str | None = None
        # For scroll
        direction: str | None = Field(default="down", description="up or down")
        amount: int | None = Field(default=500, description="Pixels to scroll")

    class ActionsRequest(BaseModel):
        actions: list[Action] = Field(min_length=1, max_length=20)

    # ---- Helpers ----

    async def cleanup_expired():
        """Remove sessions idle for more than SESSION_TIMEOUT_SECS."""
        now = time.time()
        expired = [
            sid for sid, s in _sessions.items()
            if now - s["last_active"] > SESSION_TIMEOUT_SECS
        ]
        for sid in expired:
            session = _sessions.pop(sid, None)
            if session and session.get("context"):
                try:
                    await session["context"].close()
                except Exception:
                    pass

    # ---- Endpoints ----

    @fastapi_app.get("/health")
    async def health():
        await cleanup_expired()
        return {
            "status": "ok",
            "active_sessions": len(_sessions),
            "timeout_secs": SESSION_TIMEOUT_SECS,
        }

    @fastapi_app.post("/session/create")
    async def create_session(req: CreateSessionRequest):
        await cleanup_expired()

        if len(_sessions) >= 8:
            raise HTTPException(429, "Too many active sessions")

        browser = await get_browser()
        context = await browser.new_context(
            viewport={"width": req.viewport_width, "height": req.viewport_height},
            locale=req.locale,
            user_agent=req.user_agent or (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) "
                "AppleWebKit/537.36 (KHTML, like Gecko) "
                "Chrome/131.0.0.0 Safari/537.36"
            ),
        )
        page = await context.new_page()
        session_id = str(uuid.uuid4())

        async with _lock:
            _sessions[session_id] = {
                "context": context,
                "page": page,
                "created_at": time.time(),
                "last_active": time.time(),
            }

        return {"session_id": session_id}

    @fastapi_app.post("/session/{session_id}/actions")
    async def execute_actions(session_id: str, req: ActionsRequest):
        session = _sessions.get(session_id)
        if not session:
            raise HTTPException(404, f"Session {session_id} not found")

        session["last_active"] = time.time()
        page = session["page"]
        results = []

        for action in req.actions:
            try:
                result = await _execute_single_action(page, action)
                results.append({"status": "ok", "type": action.type, **result})
            except Exception as e:
                results.append({
                    "status": "error",
                    "type": action.type,
                    "error": str(e),
                })
                # Stop batch on error
                break

        return {
            "session_id": session_id,
            "results": results,
            "page_url": page.url,
            "page_title": await page.title(),
        }

    @fastapi_app.post("/session/{session_id}/screenshot")
    async def take_screenshot(session_id: str):
        session = _sessions.get(session_id)
        if not session:
            raise HTTPException(404, f"Session {session_id} not found")

        session["last_active"] = time.time()
        page = session["page"]

        screenshot_bytes = await page.screenshot(type="jpeg", quality=75)
        screenshot_b64 = base64.b64encode(screenshot_bytes).decode("utf-8")

        return {
            "session_id": session_id,
            "screenshot": screenshot_b64,
            "page_url": page.url,
            "page_title": await page.title(),
        }

    @fastapi_app.delete("/session/{session_id}")
    async def delete_session(session_id: str):
        session = _sessions.pop(session_id, None)
        if not session:
            raise HTTPException(404, f"Session {session_id} not found")

        try:
            await session["context"].close()
        except Exception:
            pass

        return {"status": "deleted", "session_id": session_id}

    # ---- Single action executor ----

    async def _execute_single_action(page, action: Action) -> dict:
        timeout = action.timeout_ms

        match action.type:
            case "navigate":
                if not action.url:
                    raise ValueError("navigate requires 'url'")
                await page.goto(action.url, timeout=timeout, wait_until="domcontentloaded")
                return {"url": page.url}

            case "click":
                if not action.selector:
                    raise ValueError("click requires 'selector'")
                await page.click(action.selector, timeout=timeout)
                return {}

            case "fill":
                if not action.selector or action.value is None:
                    raise ValueError("fill requires 'selector' and 'value'")
                await page.fill(action.selector, action.value, timeout=timeout)
                return {}

            case "fill_credentials":
                # Credentials are pre-decrypted by Lambda and passed as plaintext
                if not action.username_selector or not action.password_selector:
                    raise ValueError(
                        "fill_credentials requires username_selector and password_selector"
                    )
                if action.encrypted_username:
                    await page.fill(
                        action.username_selector, action.encrypted_username, timeout=timeout
                    )
                if action.encrypted_password:
                    await page.fill(
                        action.password_selector, action.encrypted_password, timeout=timeout
                    )
                return {}

            case "select":
                if not action.selector or action.value is None:
                    raise ValueError("select requires 'selector' and 'value'")
                await page.select_option(action.selector, action.value, timeout=timeout)
                return {}

            case "screenshot":
                screenshot_bytes = await page.screenshot(type="jpeg", quality=75)
                screenshot_b64 = base64.b64encode(screenshot_bytes).decode("utf-8")
                return {"screenshot": screenshot_b64}

            case "wait_navigation":
                await page.wait_for_load_state("domcontentloaded", timeout=timeout)
                return {"url": page.url}

            case "wait_selector":
                if not action.selector:
                    raise ValueError("wait_selector requires 'selector'")
                await page.wait_for_selector(action.selector, timeout=timeout)
                return {}

            case "scroll":
                direction = action.direction or "down"
                amount = action.amount or 500
                delta = amount if direction == "down" else -amount
                await page.evaluate(f"window.scrollBy(0, {delta})")
                return {}

            case "evaluate":
                if not action.expression:
                    raise ValueError("evaluate requires 'expression'")
                # Safety: limit expression length
                if len(action.expression) > 2000:
                    raise ValueError("Expression too long (max 2000 chars)")
                result = await page.evaluate(action.expression)
                return {"result": str(result)[:5000]}

            case "go_back":
                await page.go_back(timeout=timeout)
                return {"url": page.url}

            case "go_forward":
                await page.go_forward(timeout=timeout)
                return {"url": page.url}

            case _:
                raise ValueError(f"Unknown action type: {action.type}")

    return fastapi_app


# ---------------------------------------------------------------------------
# Local test entrypoint
# ---------------------------------------------------------------------------
@app.local_entrypoint()
def main():
    import requests

    # Get the deployed URL from modal
    print("Browser service deployed. Test with:")
    print("  curl -X POST <URL>/session/create")
    print("  curl -X POST <URL>/session/{id}/actions -d '{...}'")
    print("  curl -X DELETE <URL>/session/{id}")
