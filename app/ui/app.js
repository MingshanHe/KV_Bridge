// KV Bridge settings window. Talks to the Rust side through Tauri commands.
const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const $ = (id) => document.getElementById(id);

let platform = null;
let settings = null;
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

function renderStatus(st) {
  if (platform === null) {
    platform = st.platform;
    document.querySelectorAll('.win-only').forEach(e => e.classList.toggle('hidden', platform !== 'windows'));
    document.querySelectorAll('.mac-only').forEach(e => e.classList.toggle('hidden', platform !== 'macos'));
    $('win-pair').classList.toggle('hidden', platform !== 'windows');
    $('mac-pair').classList.toggle('hidden', platform !== 'macos');
  }
  $('whoami').textContent = `${st.device_name} · ${platform === 'macos' ? 'Mac' : 'Windows'}` +
    (st.local_ip ? ` · ${st.local_ip}` : '');
  $('summary').textContent = st.summary;
  $('message').textContent = st.message || '';
  $('dot').className = 'dot' + (st.controlling ? ' busy' : st.connected ? ' ok' : '');
  $('ax-card').classList.toggle('hidden', st.accessibility !== false);
  $('footer').textContent = `KV Bridge ${st.version}`;

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
        confirmForget = p.id;  // second click within 4 s deletes
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

  // Windows: Macs nearby.
  if (platform === 'windows') {
    const fl = $('found-list');
    fl.replaceChildren();
    st.found.forEach(f => {
      const it = el('div', 'item');
      const info = el('div');
      info.append(el('div', 'name', f.name));
      info.append(el('div', 'meta', f.addr + (f.paired ? ' · 已配对' : '')));
      it.append(info);
      if (!f.paired) {
        const b = el('button', '', '配对');
        b.onclick = () => openPair(f.id, f.name);
        it.append(b);
      }
      fl.append(it);
    });
    $('found-empty').classList.toggle('hidden', st.found.length > 0);
  }

  // Mac: pairing code.
  if (platform === 'macos') {
    const active = !!st.pairing_code;
    $('mac-pair-idle').classList.toggle('hidden', active);
    $('mac-pair-active').classList.toggle('hidden', !active);
    if (active) {
      $('mac-code').textContent = st.pairing_code.slice(0, 3) + ' ' + st.pairing_code.slice(3);
      $('mac-code-left').textContent = fmtSeconds(st.pairing_seconds);
    }
  }
}

async function refresh() {
  try {
    renderStatus(await invoke('get_status'));
  } catch (e) {
    $('message').textContent = String(e);
  }
}

// ----- pairing (Windows) -----
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
  const code = $('pair-code').value;
  $('pair-go').disabled = true;
  $('pair-error').textContent = '正在配对…';
  try {
    const name = await invoke('pair_with', { target: pairTarget, code });
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

// ----- pairing (Mac) -----
$('mac-pair-start').onclick = async () => { await invoke('start_pairing'); refresh(); };
$('mac-pair-cancel').onclick = async (e) => { e.preventDefault(); await invoke('stop_pairing'); refresh(); };
$('ax-open').onclick = () => invoke('open_accessibility');

// ----- settings -----
function fillSettings(s) {
  settings = s;
  $('device-name').value = s.device_name;
  $('toggle-button').value = s.win.toggle_button;
  $('hotkey').checked = s.win.hotkey;
  $('sound').checked = s.win.sound;
  $('shortcuts').checked = s.mac.windows_shortcuts;
  $('layout').value = s.mac.layout;
  $('scroll').value = s.mac.scroll_speed;
  $('scroll-val').textContent = s.mac.scroll_speed;
  $('invert').checked = s.mac.invert_scroll;
  $('jis').checked = s.mac.jis;
  $('autostart').checked = s.autostart;
}

let saveTimer = null;
function scheduleSave() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(save, 300);
}
async function save() {
  if (!settings) return;
  const win = {
    toggle_button: $('toggle-button').value,
    hotkey: $('hotkey').checked,
    sound: $('sound').checked,
  };
  const mac = {
    windows_shortcuts: $('shortcuts').checked,
    layout: $('layout').value,
    scroll_speed: Number($('scroll').value),
    invert_scroll: $('invert').checked,
    jis: $('jis').checked,
  };
  try {
    await invoke('save_settings', {
      deviceName: $('device-name').value,
      win, mac,
      autostart: $('autostart').checked,
    });
    $('saved').textContent = '已保存';
    setTimeout(() => { $('saved').textContent = ''; }, 1500);
  } catch (e) {
    $('saved').textContent = String(e);
  }
}
['toggle-button', 'hotkey', 'sound', 'shortcuts', 'layout', 'invert', 'jis', 'autostart']
  .forEach(id => $(id).addEventListener('change', scheduleSave));
$('device-name').addEventListener('change', scheduleSave);
$('scroll').addEventListener('input', () => { $('scroll-val').textContent = $('scroll').value; scheduleSave(); });

(async () => {
  fillSettings(await invoke('get_settings'));
  await refresh();
  setInterval(refresh, 1000);
})();
