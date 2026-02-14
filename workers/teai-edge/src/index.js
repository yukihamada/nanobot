// teai-edge: Cloudflare Workers edge proxy
// Routes requests to the fastest healthy backend

const BACKENDS = [
  { name: 'aws-lambda', url: 'https://api.chatweb.ai', priority: 1 },
  { name: 'fly-nrt', url: 'https://nanobot-ai.fly.dev', priority: 2 },
];

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    // CORS preflight
    if (request.method === 'OPTIONS') {
      return new Response(null, {
        headers: corsHeaders(request),
      });
    }

    // Try backends in priority order
    let lastError = null;
    for (const backend of BACKENDS) {
      try {
        const backendUrl = backend.url + url.pathname + url.search;
        const resp = await fetch(backendUrl, {
          method: request.method,
          headers: proxyHeaders(request),
          body: request.method !== 'GET' && request.method !== 'HEAD'
            ? await request.clone().arrayBuffer()
            : undefined,
        });

        // If backend returned an error, try next
        if (resp.status >= 500) {
          lastError = `${backend.name}: ${resp.status}`;
          continue;
        }

        // Success — add CORS and edge headers
        const response = new Response(resp.body, resp);
        setCorsHeaders(response.headers, request);
        response.headers.set('X-Edge-Backend', backend.name);
        response.headers.set('X-Edge-Region', request.cf?.colo || 'unknown');
        return response;
      } catch (e) {
        lastError = `${backend.name}: ${e.message}`;
        continue;
      }
    }

    // All backends failed
    return new Response(
      JSON.stringify({ error: 'All backends unavailable', detail: lastError }),
      { status: 502, headers: { 'Content-Type': 'application/json', ...Object.fromEntries(corsHeaderEntries(request)) } }
    );
  },
};

function proxyHeaders(request) {
  const h = new Headers(request.headers);
  // Preserve original Host so Lambda can detect teai.io
  const originalHost = request.headers.get('host') || '';
  h.delete('host');
  h.set('X-Forwarded-Host', originalHost);
  h.set('X-Forwarded-For', request.headers.get('cf-connecting-ip') || '');
  h.set('X-Edge-Region', request.cf?.colo || 'unknown');
  return h;
}

function corsHeaders(request) {
  return Object.fromEntries(corsHeaderEntries(request));
}

function corsHeaderEntries(request) {
  const origin = request.headers.get('Origin') || '*';
  return [
    ['Access-Control-Allow-Origin', origin],
    ['Access-Control-Allow-Methods', 'GET,POST,PUT,DELETE,OPTIONS'],
    ['Access-Control-Allow-Headers', 'Content-Type, Authorization'],
    ['Access-Control-Max-Age', '86400'],
  ];
}

function setCorsHeaders(headers, request) {
  const origin = request.headers.get('Origin') || '*';
  headers.set('Access-Control-Allow-Origin', origin);
}
