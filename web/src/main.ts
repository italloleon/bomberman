import init, { Client } from './wasm/client.js';
import { combineControls, keyboardControls } from './input';
import { startRenderer, type FrameClock, insets } from './render';
import { enterMobileFullscreen, touchControls, wantsTouchControls } from './touch';
import { startUi } from './ui';

const panel = document.getElementById('panel')!;
const notice = document.getElementById('notice')!;
const NAME_KEY = 'bomber.name';
/** Per tab (sessionStorage): reloading reclaims your slot, a second tab is a new player. */
const tokenKey = (code: string) => `bomber.token.${code}`;
/** Matches the server's reconnect grace period. */
const RECONNECT_WINDOW_MS = 15_000;
/** Dev aid: `?lag=150` delays every message so the round trip takes ~150 ms longer. */
const FAKE_LAG_MS = Number(new URLSearchParams(location.search).get('lag')) || 0;
/** After a stall (tab in background) don't try to replay more than this many ticks. */
const MAX_CATCH_UP_TICKS = 5;

const route = location.pathname.match(/^\/r\/([a-z]+)\/([a-z0-9]{4})\/?$/i);
if (route) {
  const code = route[2].toUpperCase();
  const name = localStorage.getItem(NAME_KEY);
  // Coming back to a room we were already in (e.g. a reload): skip the form.
  if (name && sessionStorage.getItem(tokenKey(code))) void startGame(code, name);
  else showJoin(code);
} else {
  showHome();
}

function showHome() {
  panel.innerHTML = `
    <h1>Bomberman</h1>
    <p>Create a room and send the link to your friends.</p>
    <button id="create">Create room</button>`;
  const button = panel.querySelector<HTMLButtonElement>('#create')!;
  button.onclick = async () => {
    button.disabled = true;
    try {
      const res = await fetch('/api/rooms', { method: 'POST' });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const { region, code } = await res.json();
      location.href = `/r/${region}/${code}${location.search}`;
    } catch {
      showMessage('Could not reach the server.');
    }
  };
}

function showJoin(code: string) {
  panel.innerHTML = `
    <h1>Room ${code}</h1>
    <form id="join">
      <input id="name" maxlength="16" placeholder="Nickname" autocomplete="off" required />
      <button>Join</button>
    </form>`;
  const input = panel.querySelector<HTMLInputElement>('#name')!;
  input.value = localStorage.getItem(NAME_KEY) ?? '';
  input.focus();
  panel.querySelector('form')!.onsubmit = (e) => {
    e.preventDefault();
    const name = input.value.trim();
    localStorage.setItem(NAME_KEY, name);
    if (wantsTouchControls()) void enterMobileFullscreen(); // needs this tap as the gesture
    void startGame(code, name);
  };
}

function showMessage(text: string) {
  panel.hidden = false;
  panel.style.background = 'rgba(22, 22, 29, 0.85)';
  panel.innerHTML = `<p></p><a href="/"><button>Back to start</button></a>`;
  panel.querySelector('p')!.textContent = text;
}

/** Run `fn` after the fake one-way delay (half the extra round trip), or right away. */
const delayed = (fn: () => void) => (FAKE_LAG_MS ? setTimeout(fn, FAKE_LAG_MS / 2) : fn());

async function startGame(code: string, name: string) {
  await init(); // loads and instantiates the .wasm module
  const client = new Client();
  const controls = wantsTouchControls()
    ? combineControls(keyboardControls(), touchControls())
    : keyboardControls();
  if (wantsTouchControls()) {
    // Keep the map clear of the D-pad (left) and bomb button (right).
    const fit = () => {
      const pad = document.querySelector('.dpad')!.getBoundingClientRect();
      const bomb = document.querySelector('.bomb-button')!.getBoundingClientRect();
      insets.left = pad.right + 8;
      insets.right = window.innerWidth - bomb.left + 8;
    };
    fit();
    window.addEventListener('resize', fit);
  }
  const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
  let ws: WebSocket | null = null;
  let welcomed = false;
  let reconnectDeadline = 0; // 0 = connected (or never connected yet)

  // wasm-bindgen types returned bytes as Uint8Array<ArrayBufferLike>; they're never
  // shared memory, so narrowing to what WebSocket.send accepts is safe.
  const send = (bytes: Uint8Array) => {
    const sock = ws;
    if (!bytes.length || sock?.readyState !== WebSocket.OPEN) return;
    delayed(() => sock.readyState === WebSocket.OPEN && sock.send(bytes as Uint8Array<ArrayBuffer>));
  };

  const connect = () => {
    const sock = new WebSocket(`${scheme}://${location.host}/ws/${code}`);
    sock.binaryType = 'arraybuffer';
    ws = sock;
    welcomed = false;

    sock.onopen = () => {
      client.resetInput();
      send(client.hello(name, sessionStorage.getItem(tokenKey(code)) ?? undefined));
    };

    sock.onmessage = (e) =>
      delayed(() => {
        client.receive(new Uint8Array(e.data as ArrayBuffer), performance.now());
        const reason = client.rejected;
        if (reason) {
          sessionStorage.removeItem(tokenKey(code));
          showMessage(reason);
          return;
        }
        const token = client.token;
        if (token) sessionStorage.setItem(tokenKey(code), token);
        welcomed = client.you >= 0;
        reconnectDeadline = 0;
        notice.hidden = true;
      });

    sock.onclose = () => {
      if (client.rejected || ws !== sock) return;
      welcomed = false;
      reconnectDeadline ||= Date.now() + RECONNECT_WINDOW_MS;
      if (Date.now() < reconnectDeadline) {
        notice.textContent = 'Connection lost — reconnecting…';
        notice.hidden = false;
        setTimeout(connect, 1000);
      } else {
        notice.hidden = true;
        showMessage('Disconnected from the server.');
      }
    };
  };

  // Fixed-rate game ticks driven by animation frames: every TICK_MS of real time we
  // read the keys, predict, and send one input. `clock.alpha` tells the renderer how
  // far we are towards the next tick, so motion is smooth at any refresh rate.
  const clock: FrameClock = { alpha: 0 };
  let last = performance.now();
  let acc = 0;
  const frame = (now: number) => {
    acc = Math.min(acc + (now - last), client.tickMs * MAX_CATCH_UP_TICKS);
    last = now;
    while (acc >= client.tickMs) {
      acc -= client.tickMs;
      if (welcomed) send(client.tick(controls.dir(), controls.takeBomb(), now));
    }
    clock.alpha = acc / client.tickMs;
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);

  connect();
  await startRenderer(client, clock);
  panel.hidden = true;
  startUi(code, client, send);
}
