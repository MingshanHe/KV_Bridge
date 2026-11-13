// KV Bridge settings window. Talks to the Rust side through Tauri commands.
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const $ = (id) => document.getElementById(id);

let platform = null;
let settings = null;     // what get_settings returned, edited in place
let pairTarget = null;
let confirmForget = null;

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function fmtSeconds(s) {
  const m = Math.floor(s / 60), r = s % 60;
  return m > 0 ? `${m} 分 ${r} 秒` : `${r} 秒`;
}

// ----- status -----
function renderStatus(st) {
  if (platform === null) {
    platform = st.platform;
    document.querySelectorAll('.win-only').forEach(e => e.classList.toggle('hidden', platform !== 'windows'));
    document.querySelectorAll('.mac-only').forEach(e => e.classList.toggle('hidden', platform !== 'macos'));
    $('found-title').textContent = platform === 'windows' ? '附近的 Mac' : '附近的 Windows 电脑';
  }
  $('whoami').textContent = `${st.device_name} · ${platform === 'macos' ? 'Mac' : 'Windows'}` +
    (st.local_ip ? ` · ${st.local_ip}` : '');
  $('summary').textContent = st.summary;
  $('message').textContent = st.message || '';
  $('listen-error').textContent = st.listen_error || '';
  $('dot').className = 'dot' + (st.controlling ? ' busy' : st.connected ? ' ok' : '');
  $('ax-card').classList.toggle('hidden', st.accessibility !== false);
  $('footer').textContent = `KV Bridge ${st.version}`;
  $('code-me').textContent = st.device_name;

  $('update-card').classList.toggle('hidden', !st.update_available);
  if (st.update_available) {
    $('update-version').textContent = st.update_available;
    if (st.update_status) $('update-status').textContent = st.update_status;
  }

  // Paired computers.
  const pl = $('peer-list');
  pl.replaceChildren();
  st.peers.forEach(p => {
    const it = el('div', 'item');
    const info = el('div');
    info.append(el('div', 'name', p.name));
    const online = st.connected === p.name;
    info.append(el('div', 'meta', (online ? '已连接' : '未连接') + (p.last_addr ? ` · ${p.last_addr}` : '')));
    const b = el('button', 'secondary', confirmForget === p.id ? '确认删除？' : '删除');
    b.onclick = async () => {
      if (confirmForget !== p.id) {
        confirmForget = p.id;  // a second click within 4 s deletes
        setTimeout(() => { if (confirmForget === p.id) { confirmForget = null; refresh(); } }, 4000);
        refresh();
        return;
      }
      confirmForget = null;
      await invoke('forget_peer', { id: p.id });
      refresh();
    };
    it.append(info, b);
    pl.append(it);
  });
  $('peer-empty').classList.toggle('hidden', st.peers.length > 0);

  // Computers nearby.
  const fl = $('found-list');
  fl.replaceChildren();
  st.found.forEach(f => {
    const it = el('div', 'item');
    const info = el('div');
    info.append(el('div', 'name', f.name));
    info.append(el('div', 'meta', f.addr + (f.paired ? ' · 已配对' : '')));
    const b = el('button', f.paired ? 'secondary' : '', f.paired ? '重新配对' : '配对');
    b.onclick = () => openPair(f.id, f.name);
    it.append(info, b);
    fl.append(it);
  });
  $('found-empty').classList.toggle('hidden', st.found.length > 0);

  // Our own pairing code.
  const active = !!st.pairing_code;
  $('code-idle').classList.toggle('hidden', active);
  $('code-active').classList.toggle('hidden', !active);
  if (active) {
    $('code').textContent = st.pairing_code.slice(0, 3) + ' ' + st.pairing_code.slice(3);
    $('code-left').textContent = fmtSeconds(st.pairing_seconds);
  }
}

async function refresh() {
  try {
    renderStatus(await invoke('get_status'));
  } catch (e) {
    $('message').textContent = String(e);
  }
}

// ----- pairing -----
function openPair(target, name) {
  pairTarget = target;
  $('pair-target-name').textContent = name;
  $('pair-error').textContent = '';
  $('pair-code').value = '';
  $('pair-dialog').classList.remove('hidden');
  $('pair-code').focus();
}
$('pair-cancel').onclick = () => $('pair-dialog').classList.add('hidden');
$('manual-pair').onclick = () => {
  const ip = $('manual-ip').value.trim();
  if (ip) openPair(ip, ip);
};
async function doPair() {
  $('pair-go').disabled = true;
  $('pair-error').textContent = '正在配对…';
  try {
    const name = await invoke('pair_with', { target: pairTarget, code: $('pair-code').value });
    $('pair-error').textContent = '';
    $('pair-dialog').classList.add('hidden');
    $('message').textContent = `已和 ${name} 配对`;
  } catch (e) {
    $('pair-error').textContent = String(e);
  } finally {
    $('pair-go').disabled = false;
    refresh();
  }
}
$('pair-go').onclick = doPair;
$('pair-code').addEventListener('keydown', e => { if (e.key === 'Enter') doPair(); });
$('code-start').onclick = async () => { await invoke('start_pairing'); refresh(); };
$('code-cancel').onclick = async (e) => { e.preventDefault(); await invoke('stop_pairing'); refresh(); };
$('ax-open').onclick = () => invoke('open_accessibility');

// ----- recording a key combination (Windows) -----
// Resolves to {mods, key}, or null if cancelled (Esc) or timed out.
async function record(labelEl) {
  const before = labelEl.textContent;
  labelEl.textContent = '请按下组合键…（Esc 取消）';
  labelEl.classList.add('recording');
  try {
    await invoke('start_recording');
    const until = Date.now() + 20000;
    while (Date.now() < until) {
      await new Promise(r => setTimeout(r, 150));
      const got = await invoke('take_recorded');
      if (got) {
        const [mods, key] = got;
        if (key === 0x29 && mods === 0) return null;  // Esc
        return { mods, key };
      }
    }
    await invoke('cancel_recording');
    return null;
  } finally {
    labelEl.classList.remove('recording');
    labelEl.textContent = before;
  }
}

// ----- switch hotkey -----
function renderHotkey() {
  const hk = settings.win.switch_hotkey;
  $('hotkey-label').textContent = hk ? winCombo(hk.mods, hk.key) : '不用快捷键';
}
$('hotkey-record').onclick = async () => {
  const c = await record($('hotkey-label'));
  if (c) {
    settings.win.switch_hotkey = c;
    renderHotkey();
    save();
  }
};
$('hotkey-clear').onclick = () => {
  if ($('toggle-button').value === 'none') {
    $('saved').textContent = '鼠标按键和快捷键不能都关掉，已保留快捷键';
    return;
  }
  settings.win.switch_hotkey = null;
  renderHotkey();
  save();
};

// ----- custom shortcuts -----
let draft = null;  // rule being edited

function ruleText(r) {
  const to = r.action === 'input' ? '切换输入法' : macCombo(r.to_mods, r.to_key);
  return `${winCombo(r.from.mods, r.from.key)}  →  ${to}`;
}

function renderRules() {
  const list = $('rule-list');
  list.replaceChildren();
  settings.win.rules.forEach((r, i) => {
    const it = el('div', 'item');
    it.append(el('div', 'name', ruleText(r)));
    const b = el('button', 'secondary', '删除');
    b.onclick = () => { settings.win.rules.splice(i, 1); renderRules(); save(); };
    it.append(b);
    list.append(it);
  });
  $('rule-empty').classList.toggle('hidden', settings.win.rules.length > 0);
}

function addRule(r) {
  // One action per Windows combination.
  settings.win.rules = settings.win.rules.filter(x => !(x.from.mods === r.from.mods && x.from.key === r.from.key));
  settings.win.rules.push(r);
  renderRules();
  save();
}

(function fillTargets() {
  const sel = $('rule-key');
  MAC_TARGETS.forEach(h => {
    const o = el('option', '', keyName(h));
    o.value = h;
    sel.append(o);
  });
})();

$('rule-add').onclick = () => {
  draft = { from: null };
  $('rule-from').textContent = '—';
  $('rule-action').value = 'input';
  $('rule-keys').classList.add('hidden');
  ['m-ctrl', 'm-opt', 'm-shift', 'm-cmd'].forEach(id => { $(id).checked = false; });
  $('rule-key').value = String(0x2C);
  $('rule-error').textContent = '';
  $('rule-editor').classList.remove('hidden');
};
$('rule-preset').onclick = () => addRule({ from: { mods: 8, key: 0x2C }, action: 'input', to_mods: 0, to_key: 0 });
$('rule-cancel').onclick = () => $('rule-editor').classList.add('hidden');
$('rule-action').onchange = () => $('rule-keys').classList.toggle('hidden', $('rule-action').value !== 'keys');
$('rule-record').onclick = async () => {
  const c = await record($('rule-from'));
  if (c) {
    draft.from = c;
    $('rule-from').textContent = winCombo(c.mods, c.key);
  }
};
$('rule-save').onclick = () => {
  if (!draft || !draft.from) {
    $('rule-error').textContent = '请先录制 Windows 上的组合键';
    return;
  }
  const action = $('rule-action').value;
  const to_mods = ($('m-ctrl').checked ? 1 : 0) | ($('m-shift').checked ? 2 : 0) |
    ($('m-opt').checked ? 4 : 0) | ($('m-cmd').checked ? 8 : 0);
  addRule({ from: draft.from, action, to_mods: action === 'keys' ? to_mods : 0,
    to_key: action === 'keys' ? Number($('rule-key').value) : 0 });
  $('rule-editor').classList.add('hidden');
};

// ----- settings -----
function fillSettings(s) {
  settings = s;
  $('device-name').value = s.device_name;
  $('toggle-button').value = s.win.toggle_button;
  $('sound').checked = s.win.sound;
  $('shortcuts').checked = s.mac.windows_shortcuts;
  $('layout').value = s.mac.layout;
  $('scroll').value = s.mac.scroll_speed;
  $('scroll-val').textContent = s.mac.scroll_speed;
  $('invert').checked = s.mac.invert_scroll;
  $('jis').checked = s.mac.jis;
  $('autostart').checked = s.autostart;
  renderHotkey();
  renderRules();
}

let saveTimer = null;
function scheduleSave() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(save, 300);
}
async function save() {
  if (!settings) return;
  settings.win.toggle_button = $('toggle-button').value;
  settings.win.sound = $('sound').checked;
  if (settings.win.toggle_button === 'none' && !settings.win.switch_hotkey) {
    settings.win.switch_hotkey = { mods: 5, key: 0x10 };  // Ctrl+Alt+M
    renderHotkey();
  }
  settings.mac = {
    windows_shortcuts: $('shortcuts').checked,
    layout: $('layout').value,
    scroll_speed: Number($('scroll').value),
    invert_scroll: $('invert').checked,
    jis: $('jis').checked,
  };
  try {
    await invoke('save_settings', {
      deviceName: $('device-name').value,
      win: settings.win,
      mac: settings.mac,
      autostart: $('autostart').checked,
    });
    $('saved').textContent = '已保存';
    setTimeout(() => { $('saved').textContent = ''; }, 1500);
  } catch (e) {
    $('saved').textContent = String(e);
  }
}
['toggle-button', 'sound', 'shortcuts', 'layout', 'invert', 'jis', 'autostart']
  .forEach(id => $(id).addEventListener('change', scheduleSave));
$('device-name').addEventListener('change', scheduleSave);
$('scroll').addEventListener('input', () => { $('scroll-val').textContent = $('scroll').value; scheduleSave(); });

// ----- updates -----
$('update-go').onclick = async () => {
  $('update-go').disabled = true;
  $('update-status').textContent = '正在下载新版本…完成后会自动重新打开';
  try {
    await invoke('install_update');
  } catch (e) {
    $('update-status').textContent = String(e);
    $('update-go').disabled = false;
  }
};
$('check-update').onclick = async (e) => {
  e.preventDefault();
  $('check-result').textContent = '正在检查…';
  try {
    const v = await invoke('check_update');
    $('check-result').textContent = v ? `有新版本 ${v}` : '已经是最新版本';
  } catch (err) {
    $('check-result').textContent = String(err);
  }
  refresh();
};

(async () => {
  fillSettings(await invoke('get_settings'));
  await refresh();
  setInterval(refresh, 1000);
})();
