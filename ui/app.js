// Frontend for the tray window. Everything goes through Tauri's IPC:
//   window.__TAURI__.core.invoke  -> the commands in src-tauri/src/lib.rs
//   window.__TAURI__.event.listen -> proxy://status, proxy://notification, proxy://export
//
// There is no polling: the Rust core forwards the proxy's own 1 s stats.tick.
// Nothing here talks to the proxy directly — the core owns that channel.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

/** Keys that live alongside models in a [models.*] category but are not models. */
const RESERVED_CATEGORY_KEYS = new Set(['upstream_mode', 'base_url']);

/** Whether the proxy is running, as of the last status render. */
let running = false;

/** Lines kept in the LOG section; older lines are dropped so a chatty proxy
 *  cannot grow the DOM without bound. */
const LOG_LINES = 500;
const logLines = [];

/** Show an error. Failures are never swallowed — they land on screen. */
function fail(where, err) {
  const message = `${where}: ${err && err.message ? err.message : err}`;
  console.error(message);
  $('error').textContent = message;
  $('error').hidden = false;
}

function clearError() {
  $('error').hidden = true;
}

/** Invoke a command, surfacing a rejection instead of losing it. */
async function run(where, command, args) {
  clearError();
  try {
    await invoke(command, args);
  } catch (err) {
    fail(where, err);
  }
}

function renderStatus(status) {
  running = Boolean(status && status.running);
  const reloadError = (status && status.reloadError) || '';
  const error = (status && status.error) || '';

  $('dot').className = `dot ${error || reloadError ? 'error' : running ? 'running' : 'stopped'}`;
  $('headline').textContent = running ? `Running on :${status.port}` : 'Stopped';
  $('endpoint').textContent = [
    status && status.pid ? `pid ${status.pid}` : null,
    status && status.uptimeMs !== undefined ? `up ${Math.round(status.uptimeMs / 1000)}s` : null,
    status && status.version ? `Ver ${status.version}` : null,
  ]
    .filter(Boolean)
    .join(' · ');

  $('toggle').textContent = running ? 'Stop' : 'Start';

  if (status) {
    $('config-path').textContent = configPathText(status);
  }
  if (status && status.activeRequests !== undefined) {
    $('active').textContent = status.activeRequests;
  }
  if (error) {
    fail('status', error);
  } else if (reloadError) {
    fail('reload config', reloadError);
  }

  $('reload-error').textContent = reloadError;
  $('reload-error').hidden = !reloadError;
}

/**
 * What the Config section shows: the path handed to the proxy, or — when none
 * was resolved — the two places the proxy's own resolver would look instead
 * (doc §7), so the user knows where to drop a config.
 */
function configPathText(status) {
  if (status.configPath) {
    return status.configPath;
  }
  const candidates = [
    status.cwd ? `${status.cwd}/proxy_config.toml` : null,
    status.homeConfigPath || '~/.config/model-proxy-v3/proxy_config.toml',
  ].filter(Boolean);
  return ['not configured — the proxy will look in:', ...candidates].join('\n');
}

/** Models in [models.*], summed across categories; aliases are separate. */
function countModels(payload) {
  let models = 0;
  for (const category of Object.values(payload.models || {})) {
    models += Object.keys(category).filter((key) => !RESERVED_CATEGORY_KEYS.has(key)).length;
  }
  const aliases =
    Object.keys(payload.composite || {}).length + Object.keys(payload.schedule || {}).length;
  return { models, aliases };
}

async function refresh() {
  try {
    const status = await invoke('proxy_status');
    renderStatus(status);

    if (!status.running) {
      $('models').textContent = '—';
      return;
    }

    const payload = await invoke('rpc_call', { method: 'models.list', params: {} });
    const { models, aliases } = countModels(payload);
    $('models').textContent = aliases ? `${models} + ${aliases}` : String(models);
    $('models').title = `${models} models, ${aliases} aliases`;
  } catch (err) {
    fail('refresh', err);
  }
}

/**
 * The proxy's poll-and-diff emits one `config.changed` on its first tick even
 * though nothing changed (its previous mtime starts at -1), so the first one is
 * not a change and must not be reported as one.
 */
let sawFirstConfigTick = false;

function onNotification(frame) {
  const params = frame.params || {};
  switch (frame.method) {
    case 'stats.tick':
      $('active').textContent = params.activeRequests;
      $('tokens').textContent = Number(params.tokensTotal).toLocaleString();
      break;
    case 'config.changed': {
      if (!sawFirstConfigTick) {
        sawFirstConfigTick = true;
        break;
      }
      // The running proxy still holds the old config until Reload is pressed.
      const at = new Date(params.mtime).toLocaleTimeString();
      $('config-note').textContent = `${params.path} changed on disk at ${at} — press Reload config`;
      $('config-note').hidden = false;
      break;
    }
    default:
      console.debug('unhandled notification', frame.method, params);
  }
}

function onExport(event) {
  const { kind, output, error } = event.payload;
  const el = $('export-output');
  el.textContent = output || '(no output)';
  el.classList.remove('empty');
  if (error) {
    fail(`export (${kind})`, error);
  }
}

/** Append one line to the LOG section, capped and scrolled to the bottom. */
function appendLog(line) {
  logLines.push(line);
  if (logLines.length > LOG_LINES) {
    logLines.splice(0, logLines.length - LOG_LINES);
  }
  const el = $('log');
  el.classList.remove('empty');
  el.textContent = logLines.join('\n');
  el.scrollTop = el.scrollHeight;
}

/** Copy a section's text to the clipboard. Failures are never swallowed. */
async function copySection(where, el) {
  if (!navigator.clipboard || !navigator.clipboard.writeText) {
    fail(where, 'clipboard API unavailable');
    return;
  }
  try {
    await navigator.clipboard.writeText(el.textContent);
  } catch (err) {
    fail(where, err);
  }
}

/** Empty a section back to its placeholder and collapse it (`.empty`). */
function clearSection(el, placeholder) {
  el.textContent = placeholder;
  el.classList.add('empty');
}

$('toggle').addEventListener('click', () =>
  run('toggle', running ? 'proxy_stop' : 'proxy_start'),
);
$('restart').addEventListener('click', () => run('restart', 'proxy_restart'));
$('reload').addEventListener('click', () => run('reload config', 'reload_config'));
$('dashboard').addEventListener('click', () => run('open dashboard', 'open_dashboard'));
$('export-pi').addEventListener('click', () => run('export (pi)', 'export_provider', { kind: 'pi' }));
$('export-openclaw').addEventListener('click', () =>
  run('export (openclaw)', 'export_provider', { kind: 'openclaw' }),
);

$('export-copy').addEventListener('click', () => copySection('copy export', $('export-output')));
$('export-clear').addEventListener('click', () =>
  clearSection($('export-output'), 'No export yet.'),
);
$('log-copy').addEventListener('click', () => copySection('copy log', $('log')));
$('log-clear').addEventListener('click', () => {
  // Drop the buffer too, so the next line starts the log fresh rather than
  // re-appending lines the user just cleared.
  logLines.length = 0;
  clearSection($('log'), 'No log yet.');
});

listen('proxy://status', (event) => {
  renderStatus(event.payload);
  // A start/stop changes what models.list would answer, so re-read it.
  refresh();
});
listen('proxy://notification', (event) => onNotification(event.payload));
listen('proxy://export', onExport);
listen('proxy://log', (event) => appendLog(event.payload.line));

refresh();
