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
const LOG_LINES = 100;
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

/** Format uptime in milliseconds to a human-readable string with h/m/s/ms units. */
function formatUptime(uptimeMs) {
  if (uptimeMs < 1000) {
    return 'just started';
  }
  const totalSeconds = Math.floor(uptimeMs / 1000);
  if (totalSeconds < 60) {
    return `up ${totalSeconds}s`;
  }
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  if (minutes < 60) {
    return seconds > 0 ? `up ${minutes}m ${seconds}s` : `up ${minutes}m`;
  }
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return remainingMinutes > 0 ? `up ${hours}h ${remainingMinutes}m` : `up ${hours}h`;
}

function renderStatus(status) {
  running = Boolean(status && status.running);
  const reloadError = (status && status.reloadError) || '';
  const error = (status && status.error) || '';
  const activeRequests = status && status.activeRequests ? Number(status.activeRequests) : 0;
  const isServing = running && activeRequests > 0;

  $('dot').className = `dot ${error || reloadError ? 'error' : running ? 'running' : 'stopped'}${isServing ? ' serving' : ''}`;
  $('headline').textContent = running ? `Proxy running on: ${status.port}` : 'Stopped';
  $('endpoint').textContent = [
    status && status.pid ? `pid ${status.pid}` : null,
    status && status.uptimeMs !== undefined ? formatUptime(status.uptimeMs) : null,
    status && status.version ? `(ver ${status.version})` : null,
  ]
    .filter(Boolean)
    .join(' · ');

  // Update toggle button with appropriate icon and text
  const toggleBtn = $('toggle');
  if (running) {
    toggleBtn.innerHTML = `
      <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><rect x="6" y="6" width="12" height="12" rx="2"/></svg>
      Stop
    `;
  } else {
    toggleBtn.innerHTML = `
      <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><polygon points="5 3 19 12 5 21 5 3"/></svg>
      Start
    `;
  }

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

/** Normalize path separators to forward slashes for consistent display. */
function normalizePath(path) {
  return path.replace(/\\/g, '/');
}

/**
 * What the Config section shows: the path handed to the proxy, or — when none
 * was resolved — the two places the proxy's own resolver would look instead
 * (doc §7), so the user knows where to drop a config.
 */
function configPathText(status) {
  if (status.configPath) {
    return normalizePath(status.configPath);
  }
  const candidates = [
    status.cwd ? `${normalizePath(status.cwd)}/proxy_config.toml` : null,
    status.homeConfigPath ? normalizePath(status.homeConfigPath) : '~/.config/model-proxy-v3/proxy_config.toml',
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
    case 'stats.tick': {
      const activeRequests = Number(params.activeRequests);
      $('active').textContent = activeRequests;
      $('tokens').textContent = Number(params.tokensTotal).toLocaleString();
      // Update the serving indicator on the dot
      const dot = $('dot');
      if (running && activeRequests > 0) {
        dot.classList.add('serving');
      } else {
        dot.classList.remove('serving');
      }
      break;
    }
    case 'config.changed': {
      if (!sawFirstConfigTick) {
        sawFirstConfigTick = true;
        break;
      }
      // The running proxy still holds the old config until Reload is pressed.
      const at = new Date(params.mtime).toLocaleTimeString();
      $('config-note').textContent = `${normalizePath(params.path)} changed on disk at ${at} — press Reload config`;
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
$('export-pi').addEventListener('click', () => run('export (Pi)', 'export_provider', { kind: 'pi' }));
$('export-openclaw').addEventListener('click', () =>
  run('export (OpenClaw)', 'export_provider', { kind: 'openclaw' }),
);
$('export-dsh').addEventListener('click', () =>
  run('export (DSH)', 'export_provider', { kind: 'dsh' }),
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

// Model dropdown logic
let modelsCache = [];

async function loadModels() {
  if (!running) {
    $('model-select').disabled = true;
    $('model-select').innerHTML = '<option value="">Proxy not running</option>';
    $('test-model').disabled = true;
    return;
  }

  try {
    $('models-refresh').disabled = true;
    $('models-refresh').textContent = 'Loading…';

    const payload = await invoke('rpc_call', { method: 'models.list', params: {} });
    modelsCache = [];

    // Flatten all models from all categories
    for (const [categoryName, category] of Object.entries(payload.models || {})) {
      if (!category || Array.isArray(category)) continue;
      for (const [modelKey, modelValue] of Object.entries(category)) {
        if (RESERVED_CATEGORY_KEYS.has(modelKey)) continue;
        const alias = Array.isArray(modelValue) ? modelValue[0] || '' : (modelValue || '');
        const base = Array.isArray(modelValue) ? modelValue[1] || '' : '';
        modelsCache.push({ id: modelKey, alias, base, category: categoryName });
      }
    }

    // Also include composite aliases
    for (const [aliasName, targets] of Object.entries(payload.composite || {})) {
      modelsCache.push({ id: aliasName, alias: aliasName, base: '', category: 'composite', isAlias: true });
    }

    // And schedule aliases
    for (const [aliasName, targets] of Object.entries(payload.schedule || {})) {
      modelsCache.push({ id: aliasName, alias: aliasName, base: '', category: 'schedule', isAlias: true });
    }

    populateModelSelect();
    $('model-select').disabled = modelsCache.length === 0;
    $('test-model').disabled = modelsCache.length === 0;
  } catch (err) {
    fail('load models', err);
    $('model-select').disabled = true;
    $('model-select').innerHTML = '<option value="">Failed to load models</option>';
    $('test-model').disabled = true;
  } finally {
    $('models-refresh').disabled = false;
    $('models-refresh').textContent = 'Refresh';
  }
}

function populateModelSelect() {
  const select = $('model-select');
  const currentValue = select.value;
  select.innerHTML = '<option value="">Select a model…</option>';

  for (const model of modelsCache) {
    const opt = document.createElement('option');
    opt.value = model.id;
    const label = model.alias && model.alias !== model.id ? `${model.id} (${model.alias})` : model.id;
    opt.textContent = `[${model.category}] ${label}`;
    select.appendChild(opt);
  }

  // Restore selection if still valid
  if (currentValue && modelsCache.some(m => m.id === currentValue)) {
    select.value = currentValue;
  }
}

async function testSelectedModel() {
  const modelId = $('model-select').value;
  if (!modelId) return;

  const btn = $('test-model');
  const resultEl = $('model-test-result');

  btn.disabled = true;
  btn.textContent = 'Testing…';
  resultEl.hidden = true;
  resultEl.textContent = '';
  resultEl.className = 'empty';

  const startTime = Date.now();
  const timerInterval = setInterval(() => {
    const elapsed = ((Date.now() - startTime) / 1000).toFixed(1);
    resultEl.hidden = false;
    resultEl.className = 'testing';
    resultEl.textContent = `Testing ${modelId}… ${elapsed}s`;
  }, 100);

  try {
    const result = await invoke('rpc_call', {
      method: 'model.test',
      params: { modelId }
    });

    clearInterval(timerInterval);
    const elapsedSec = ((Date.now() - startTime) / 1000).toFixed(1);

    resultEl.hidden = false;
    resultEl.classList.remove('empty', 'testing');
    if (result.success) {
      resultEl.className = 'success';
      let msg = `✓ ${modelId}: ${result.detail || 'OK'} (${elapsedSec}s)`;
      const u = result.usage;
      if (u && typeof u.prompt_tokens === 'number' && typeof u.completion_tokens === 'number') {
        const total = u.total_tokens ?? (u.prompt_tokens + u.completion_tokens);
        msg += ` — ${u.prompt_tokens} prompt + ${u.completion_tokens} completion = ${total} tokens`;
      }
      resultEl.textContent = msg;
    } else {
      resultEl.className = 'error';
      resultEl.textContent = `✗ ${modelId}: ${result.detail || result.status || 'Failed'} (${elapsedSec}s)`;
    }
  } catch (err) {
    clearInterval(timerInterval);
    const elapsedSec = ((Date.now() - startTime) / 1000).toFixed(1);
    resultEl.hidden = false;
    resultEl.classList.remove('empty', 'testing');
    resultEl.className = 'error';
    resultEl.textContent = `✗ ${modelId}: ${err && err.message ? err.message : err} (${elapsedSec}s)`;
  } finally {
    btn.disabled = false;
    btn.textContent = 'Test';
  }
}

$('models-refresh').addEventListener('click', loadModels);
$('model-select').addEventListener('change', () => {
  $('test-model').disabled = !$('model-select').value;
});
$('test-model').addEventListener('click', testSelectedModel);

listen('proxy://status', (event) => {
  renderStatus(event.payload);
  // A start/stop changes what models.list would answer, so re-read it.
  refresh();
  loadModels();
});
listen('proxy://notification', (event) => onNotification(event.payload));
listen('proxy://export', onExport);
listen('proxy://log', (event) => appendLog(event.payload.line));

refresh();
