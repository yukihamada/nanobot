// nanobot-edge: Cloudflare Workers edge proxy
//
// Traffic flow:
//   User → CF Workers (DDoS protection, edge cache, CORS)
//        → Fly.io nanobot-ai.fly.dev (primary — libSQL backend)
//        → AWS Lambda api.chatweb.ai  (fallback — DynamoDB backend)
//
// SSE routes (/stream, /race, /explore): no cache, direct pass-through
// Static GET routes: 60s edge cache

/** Backend priority list. First healthy backend wins. */
const BACKENDS = [
  { name: 'fly-nrt',    url: 'https://nanobot-ai.fly.dev',  priority: 1 },
  { name: 'aws-lambda', url: 'https://api.chatweb.ai',       priority: 2 },
];

/** Routes that must never be cached (SSE streaming). */
const SSE_PATHS = ['/api/v1/chat/stream', '/api/v1/chat/race', '/api/v1/chat/explore'];

/** Routes eligible for short edge cache (GET only). */
const CACHE_TTL_SECS = 60;

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);

    // CORS preflight
    if (request.method === 'OPTIONS') {
      return new Response(null, { headers: corsHeaders(request) });
    }

    const isSSE = SSE_PATHS.some(p => url.pathname.startsWith(p));
    const isGet = request.method === 'GET';

    // Edge cache for safe GET requests (non-SSE)
    if (isGet && !isSSE) {
      const cache = caches.default;
      const cacheKey = new Request(url.toString(), request);
      const cached = await cache.match(cacheKey);
      if (cached) {
        const resp = new Response(cached.body, cached);
        resp.headers.set('X-Edge-Cache', 'HIT');
        setCorsHeaders(resp.headers, request);
        return resp;
      }

      const resp = await tryBackends(request, url, env);
      if (resp.ok || resp.status === 304) {
        const cacheResp = resp.clone();
        cacheResp.headers.set('Cache-Control', `public, max-age=${CACHE_TTL_SECS}`);
        ctx.waitUntil(cache.put(cacheKey, cacheResp));
      }
      resp.headers.set('X-Edge-Cache', 'MISS');
      setCorsHeaders(resp.headers, request);
      return resp;
    }

    // SSE and mutating requests — direct pass-through
    const resp = await tryBackends(request, url, env);
    setCorsHeaders(resp.headers, request);
    return resp;
  },
};

// ---------------------------------------------------------------------------
// Backend selection
// ---------------------------------------------------------------------------

async function tryBackends(request, url, env) {
  // Allow env vars to override backend URLs (e.g. for blue/green deploys)
  const backends = BACKENDS.map(b => ({
    ...b,
    url: (env && env[b.name.toUpperCase().replace(/-/g, '_') + '_URL']) || b.url,
  }));

  let lastError = null;

  for (const backend of backends) {
    try {
      const backendUrl = backend.url + url.pathname + url.search;
      const resp = await fetch(backendUrl, {
        method: request.method,
        headers: proxyHeaders(request, backend.name),
        body:
          request.method !== 'GET' && request.method !== 'HEAD'
            ? await request.clone().arrayBuffer()
            : undefined,
        // Don't follow redirects — pass 3xx to browser (OAuth etc.)
        redirect: 'manual',
        cf: { cacheEverything: false },
      });

      if (resp.status >= 500) {
        lastError = `${backend.name}: HTTP ${resp.status}`;
        continue;
      }

      const response = new Response(resp.body, {
        status: resp.status,
        statusText: resp.statusText,
        headers: new Headers(resp.headers),
      });
      response.headers.set('X-Edge-Backend', backend.name);
      response.headers.set('X-Edge-Region', request.cf?.colo ?? 'unknown');
      return response;
    } catch (e) {
      lastError = `${backend.name}: ${e.message}`;
      continue;
    }
  }

  // All backends failed
  return new Response(
    JSON.stringify({ error: 'All backends unavailable', detail: lastError }),
    {
      status: 502,
      headers: {
        'Content-Type': 'application/json',
        ...Object.fromEntries(corsHeaderEntries(request)),
      },
    }
  );
}

// ---------------------------------------------------------------------------
// Header helpers
// ---------------------------------------------------------------------------

function proxyHeaders(request, backendName) {
  const h = new Headers(request.headers);
  const originalHost = request.headers.get('host') ?? '';
  h.delete('host');
  h.set('X-Forwarded-Host', originalHost);
  h.set('X-Forwarded-For', request.headers.get('cf-connecting-ip') ?? '');
  h.set('X-Edge-Region', request.cf?.colo ?? 'unknown');
  h.set('X-Edge-Backend', backendName);
  return h;
}

function corsHeaders(request) {
  return Object.fromEntries(corsHeaderEntries(request));
}

function corsHeaderEntries(request) {
  const origin = request.headers.get('Origin') ?? '*';
  return [
    ['Access-Control-Allow-Origin', origin],
    ['Access-Control-Allow-Methods', 'GET,POST,PUT,DELETE,OPTIONS'],
    ['Access-Control-Allow-Headers', 'Content-Type, Authorization'],
    ['Access-Control-Max-Age', '86400'],
  ];
}

function setCorsHeaders(headers, request) {
  const origin = request.headers.get('Origin') ?? '*';
  headers.set('Access-Control-Allow-Origin', origin);
}
