// Key names by USB HID usage (the codes keys travel as), and modifier labels.
const KEY_NAMES = (() => {
  const k = {};
  for (let i = 0; i < 26; i++) k[0x04 + i] = String.fromCharCode(65 + i);
  '1234567890'.split('').forEach((c, i) => { k[0x1E + i] = c; });
  Object.assign(k, {
    0x28: 'Enter', 0x29: 'Esc', 0x2A: 'Backspace', 0x2B: 'Tab', 0x2C: 'Space',
    0x2D: '-', 0x2E: '=', 0x2F: '[', 0x30: ']', 0x31: '#  (\\)', 0x33: ';', 0x34: "'",
    0x35: '`', 0x36: ',', 0x37: '.', 0x38: '/', 0x39: 'Caps Lock',
    0x46: 'Print Screen', 0x47: 'Scroll Lock', 0x48: 'Pause', 0x49: 'Insert',
    0x4A: 'Home', 0x4B: 'Page Up', 0x4C: 'Delete', 0x4D: 'End', 0x4E: 'Page Down',
    0x4F: '→', 0x50: '←', 0x51: '↓', 0x52: '↑', 0x53: 'Num Lock',
    0x54: 'Num /', 0x55: 'Num *', 0x56: 'Num -', 0x57: 'Num +', 0x58: 'Num Enter',
    0x63: 'Num .', 0x64: '\\  (ISO)', 0x65: 'Menu',
    0x87: 'ろ', 0x88: 'カタカナ', 0x89: '¥', 0x8A: '変換', 0x8B: '無変換',
    0x90: 'かな (Mac)', 0x91: '英数 (Mac)',
  });
  for (let i = 0; i < 12; i++) k[0x3A + i] = 'F' + (i + 1);
  for (let i = 0; i < 8; i++) k[0x68 + i] = 'F' + (13 + i);
  for (let i = 1; i <= 9; i++) k[0x59 + i - 1] = 'Num ' + i;
  k[0x62] = 'Num 0';
  return k;
})();

function keyName(hid) {
  return KEY_NAMES[hid] || ('键 0x' + hid.toString(16));
}

// Windows modifier bits: 1 Ctrl, 2 Shift, 4 Alt, 8 Win.
function winCombo(mods, key) {
  const p = [];
  if (mods & 1) p.push('Ctrl');
  if (mods & 8) p.push('Win');
  if (mods & 4) p.push('Alt');
  if (mods & 2) p.push('Shift');
  p.push(keyName(key));
  return p.join(' + ');
}

// Mac modifier bits: 1 Control, 2 Shift, 4 Option, 8 Command.
function macCombo(mods, key) {
  let s = '';
  if (mods & 1) s += '⌃';
  if (mods & 4) s += '⌥';
  if (mods & 2) s += '⇧';
  if (mods & 8) s += '⌘';
  return s + keyName(key).replace(' (Mac)', '');
}

// Keys offered as targets on the Mac.
const MAC_TARGETS = [
  0x2C, 0x91, 0x90, 0x2B, 0x28, 0x29, 0x2A, 0x4C,
  ...Array.from({ length: 26 }, (_, i) => 0x04 + i),
  ...Array.from({ length: 10 }, (_, i) => 0x1E + i),
  0x2D, 0x2E, 0x2F, 0x30, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38,
  0x4F, 0x50, 0x51, 0x52, 0x4A, 0x4D, 0x4B, 0x4E,
  ...Array.from({ length: 12 }, (_, i) => 0x3A + i),
  ...Array.from({ length: 8 }, (_, i) => 0x68 + i),
];
