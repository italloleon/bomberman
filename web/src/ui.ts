// DOM overlays around the canvas: top bar, lobby card, phase banners.
// Everything is derived from the WASM client's state on a timer, so the UI can never
// drift from what the server says.

import type { Client } from './wasm/client.js';
import { PLAYER_COLORS } from './render';

const Phase = { Lobby: 0, Countdown: 1, Playing: 2, RoundOver: 3, MatchOver: 4 } as const;
const SUDDEN_DEATH_BANNER_MS = 2000;

const hud = document.getElementById('hud')!;
const banner = document.getElementById('banner')!;
const lobby = document.getElementById('lobby')!;

export const colorOf = (id: number) => `#${PLAYER_COLORS[id].toString(16).padStart(6, '0')}`;

function nameEl(client: Client, id: number, suffix = ''): HTMLSpanElement {
  const el = document.createElement('span');
  el.textContent = client.playerName(id) + suffix;
  el.style.color = colorOf(id);
  if (!client.playerConnected(id)) el.classList.add('away');
  return el;
}

/** Mount the overlays and keep them in sync. `send` delivers bytes to the server. */
export function startUi(code: string, client: Client, send: (bytes: Uint8Array) => void): void {
  hud.hidden = false;
  hud.innerHTML = `
    <span>Room <b>${code}</b></span>
    <button id="copy">Copy link</button>
    <span class="names"></span>
    <span class="stats"></span>
    <span class="timer"></span>
    <span class="ping"></span>`;
  const copy = hud.querySelector<HTMLButtonElement>('#copy')!;
  copy.onclick = async () => {
    await navigator.clipboard.writeText(location.origin + location.pathname);
    copy.textContent = 'Copied!';
    setTimeout(() => (copy.textContent = 'Copy link'), 1500);
  };
  const names = hud.querySelector('.names')!;
  const stats = hud.querySelector('.stats')!;
  const timer = hud.querySelector('.timer')!;
  const ping = hud.querySelector('.ping')!;

  // Rebuild DOM only when the inputs to it change.
  let namesKey = '';
  let lobbyKey = '';
  let wasSuddenDeath = false;
  let suddenDeathAt = 0;

  const update = () => {
    const phase = client.phase;
    const scores = client.scores();
    const ids = Array.from(client.rosterIds());
    const inMatch = phase !== Phase.Lobby;

    // --- top bar: names (+ score during a match), own stats, timer ---
    const nk = [client.rosterVersion, inMatch, ...scores].join();
    if (nk !== namesKey) {
      namesKey = nk;
      names.replaceChildren(...ids.map((id) => nameEl(client, id, inMatch ? ` ★${scores[id]}` : '')));
    }

    ping.textContent = client.ping ? `${Math.round(client.ping)} ms` : '';

    const me = playerStats(client);
    stats.textContent = me && phase === Phase.Playing ? `💣${me.bombs}  🔥${me.range}  👟${me.speedUps}` : '';

    const suddenDeath = client.suddenDeath;
    if (suddenDeath && !wasSuddenDeath) suddenDeathAt = performance.now();
    wasSuddenDeath = suddenDeath;

    if (phase === Phase.Playing && suddenDeath) {
      timer.textContent = 'SUDDEN DEATH';
      timer.classList.add('urgent');
    } else if (phase === Phase.Playing) {
      const s = Math.ceil(client.phaseSecondsLeft);
      timer.textContent = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
      timer.classList.toggle('urgent', s <= 10);
    } else {
      timer.textContent = inMatch ? `Round ${client.round}` : '';
      timer.classList.remove('urgent');
    }

    // --- banner ---
    const winnerName = client.winner >= 0 ? client.playerName(client.winner) : '';
    let text = '';
    let small = false;
    if (phase === Phase.Countdown) text = String(Math.ceil(client.phaseSecondsLeft));
    else if (phase === Phase.RoundOver) text = winnerName ? `${winnerName} wins the round!` : 'Draw!';
    else if (phase === Phase.MatchOver) text = `🏆 ${winnerName} wins the match!`;
    // Matches the server's 2 s grace before the first wall falls.
    else if (suddenDeath && performance.now() - suddenDeathAt < SUDDEN_DEATH_BANNER_MS) text = 'Sudden death!';
    else if (phase === Phase.Playing && !client.inRound) {
      text = 'Spectating — you join next round';
      small = true;
    }
    banner.hidden = !text;
    banner.textContent = text;
    banner.classList.toggle('small', small);
    if (phase === Phase.RoundOver || phase === Phase.MatchOver) {
      banner.style.color = client.winner >= 0 ? colorOf(client.winner) : '#fff';
    } else {
      banner.style.color = '#fff';
    }

    // --- lobby card ---
    lobby.hidden = phase !== Phase.Lobby;
    if (phase === Phase.Lobby) {
      const lk = [client.rosterVersion, client.winsNeeded, client.host, client.you].join();
      if (lk !== lobbyKey) {
        lobbyKey = lk;
        renderLobby(client, ids, send);
      }
    }
  };

  update();
  setInterval(update, 100);
}

function renderLobby(client: Client, ids: number[], send: (bytes: Uint8Array) => void) {
  const isHost = client.host === client.you;
  const connected = ids.filter((id) => client.playerConnected(id)).length;

  lobby.innerHTML = `
    <h2>Waiting room</h2>
    <ul></ul>
    <div class="row"><span>First to</span><span class="wins"></span><span>round wins</span></div>
    <div class="actions"></div>
    <div class="muted">Up to 4 players. Send the room link to friends.</div>`;

  const list = lobby.querySelector('ul')!;
  for (const id of ids) {
    const li = document.createElement('li');
    li.append(nameEl(client, id));
    const tags = [id === client.host && 'host', id === client.you && 'you', !client.playerConnected(id) && 'reconnecting…']
      .filter(Boolean)
      .join(', ');
    if (tags) {
      const t = document.createElement('span');
      t.className = 'muted';
      t.textContent = ` (${tags})`;
      li.append(t);
    }
    list.append(li);
  }

  const wins = lobby.querySelector('.wins')!;
  const actions = lobby.querySelector('.actions')!;
  if (isHost) {
    const minus = button('−', () => send(client.setWinsNeeded(client.winsNeeded - 1)));
    const plus = button('+', () => send(client.setWinsNeeded(client.winsNeeded + 1)));
    const value = document.createElement('b');
    value.textContent = String(client.winsNeeded);
    wins.replaceChildren(minus, value, plus);
    wins.classList.add('row');

    const start = button(connected >= 2 ? 'Start match' : 'Need 2+ players', () => send(client.startMatch()));
    start.classList.add('primary');
    start.disabled = connected < 2;
    actions.append(start);
  } else {
    wins.textContent = ` ${client.winsNeeded} `;
    const host = client.host >= 0 ? client.playerName(client.host) : 'the host';
    actions.innerHTML = `<span class="muted"></span>`;
    actions.querySelector('span')!.textContent = `Waiting for ${host} to start…`;
  }
}

function button(label: string, onClick: () => void): HTMLButtonElement {
  const b = document.createElement('button');
  b.textContent = label;
  b.onclick = onClick;
  return b;
}

function playerStats(client: Client) {
  const p = client.players(performance.now(), 0);
  for (let i = 0; i < p.length; i += 7) {
    if (p[i] === client.you) return { bombs: p[i + 4], range: p[i + 5], speedUps: p[i + 6] };
  }
  return null;
}
