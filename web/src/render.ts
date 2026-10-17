import { Application, Container, Graphics, Text } from 'pixi.js';
import type { Client } from './wasm/client.js';

/** Pixels per tile in world space, before the whole world is scaled to fit the screen. */
const TILE = 32;

export const PLAYER_COLORS = [0xe5484d, 0x3e93f7, 0x46a758, 0xf5d90a];
const TILE_COLORS = [
  0x2f7a47, // floor
  0x5a5a66, // wall
  0xa8763e, // breakable block
];

/** A power-up tile at pixel (x, y): colored badge + a simple symbol. 0 bomb, 1 fire, 2 speed. */
function drawPowerUp(g: Graphics, x: number, y: number, kind: number) {
  const pad = TILE * 0.12;
  const s = TILE - pad * 2;
  const [cx, cy] = [x + TILE / 2, y + TILE / 2];
  const bg = [0x3e63dd, 0xe5484d, 0x12a594][kind];
  g.roundRect(x + pad, y + pad, s, s, 5).fill(bg).stroke({ width: 1.5, color: 0xffffff, alpha: 0.8 });
  if (kind === 0) {
    // bomb: black ball
    g.circle(cx, cy + 1, s * 0.27).fill(0x111111);
    g.circle(cx + s * 0.18, cy - s * 0.2, s * 0.07).fill(0xffa500);
  } else if (kind === 1) {
    // fire: flame-ish diamond
    g.poly([cx, cy - s * 0.34, cx + s * 0.22, cy + s * 0.05, cx, cy + s * 0.32, cx - s * 0.22, cy + s * 0.05]).fill(0xffe066);
  } else {
    // speed: double chevron
    for (const off of [-s * 0.14, s * 0.1]) {
      g.poly([
        cx + off - s * 0.1, cy - s * 0.22,
        cx + off + s * 0.1, cy,
        cx + off - s * 0.1, cy + s * 0.22,
      ]).stroke({ width: 3, color: 0xffffff, cap: 'round', join: 'round' });
    }
  }
}

/** Screen space (px) the map must leave free: top bar, and on-screen controls on touch. */
export const insets = { top: 36, left: 0, right: 0 };

/** Shared with the tick loop: how far (0..1) we are between the last tick and the next. */
export interface FrameClock {
  alpha: number;
}

function drawBomb(g: Graphics, tileX: number, tileY: number, urgency: number, alpha = 1) {
  const cx = (tileX + 0.5) * TILE;
  const cy = (tileY + 0.5) * TILE;
  // Pulse faster as the fuse burns down.
  const pulse = 1 + 0.08 * Math.sin(performance.now() / (180 - 120 * urgency));
  const r = TILE * 0.36 * pulse;
  g.circle(cx, cy, r).fill({ color: 0x1b1b22, alpha });
  g.circle(cx - r * 0.35, cy - r * 0.35, r * 0.22).fill({ color: 0xffffff, alpha: 0.35 * alpha });
  g.circle(cx + r * 0.6, cy - r * 0.75, TILE * 0.08).fill({ color: urgency > 0.75 ? 0xff3b30 : 0xffa500, alpha });
}

/** Draws whatever state the WASM client currently holds, every animation frame. */
export async function startRenderer(client: Client, clock: FrameClock): Promise<void> {
  const app = new Application();
  await app.init({ resizeTo: window, background: '#16161d', antialias: true });
  document.body.prepend(app.canvas);

  const world = new Container();
  app.stage.addChild(world);
  const mapLayer = new Graphics();
  // Flames, items and bombs are few, so they're simply redrawn from scratch every frame.
  const effectsLayer = new Graphics();
  // Bombs go above players: you're usually standing on the bomb you just placed, and it
  // must stay visible.
  const bombLayer = new Graphics();
  world.addChild(mapLayer, effectsLayer);

  const sprites = new Map<number, { root: Container; name: Text }>();
  const unitToPx = TILE / client.unitsPerTile;
  let drawnMap = 0;
  let drawnRoster = 0;

  const drawMap = () => {
    const tiles = client.mapTiles();
    const w = client.mapWidth;
    mapLayer.clear();
    tiles.forEach((t, i) => {
      const x = (i % w) * TILE;
      const y = Math.floor(i / w) * TILE;
      mapLayer.rect(x, y, TILE, TILE).fill(TILE_COLORS[t]);
      if (t !== 0) {
        // Thin darker edge so adjacent blocks read as separate tiles.
        mapLayer.rect(x + 1, y + 1, TILE - 2, TILE - 2).stroke({ width: 1, color: 0x000000, alpha: 0.25 });
      }
    });
  };

  const drawEffects = () => {
    const g = effectsLayer;
    g.clear();

    const flames = client.flames();
    for (let i = 0; i < flames.length; i += 3) {
      const [x, y, ticks] = [flames[i] * TILE, flames[i + 1] * TILE, flames[i + 2]];
      // Fade out over the flame's lifetime.
      const alpha = 0.35 + 0.65 * (ticks / client.flameTicks);
      g.rect(x, y, TILE, TILE).fill({ color: 0xff7a1a, alpha });
      g.rect(x + TILE * 0.2, y + TILE * 0.2, TILE * 0.6, TILE * 0.6).fill({ color: 0xffe066, alpha });
    }

    // Sudden death: pulse a red warning on the tile the next wall will land on.
    const next = client.nextWall();
    if (next.length) {
      const pulse = 0.35 + 0.35 * Math.sin(performance.now() / 60);
      g.rect(next[0] * TILE, next[1] * TILE, TILE, TILE).fill({ color: 0xff2d2d, alpha: pulse });
      g.rect(next[0] * TILE + 1.5, next[1] * TILE + 1.5, TILE - 3, TILE - 3).stroke({ width: 3, color: 0xff2d2d });
    }

    const items = client.items();
    for (let i = 0; i < items.length; i += 3) {
      drawPowerUp(g, items[i] * TILE, items[i + 1] * TILE, items[i + 2]);
    }

    const b = bombLayer;
    b.clear();
    const bombs = client.bombs();
    for (let i = 0; i < bombs.length; i += 3) {
      drawBomb(b, bombs[i], bombs[i + 1], 1 - bombs[i + 2] / client.bombFuseTicks);
    }
    // Bombs we placed that the server hasn't confirmed yet: shown at once, translucent.
    const ghosts = client.ghostBombs();
    for (let i = 0; i < ghosts.length; i += 2) {
      drawBomb(b, ghosts[i], ghosts[i + 1], 0, 0.5);
    }
  };

  const makeSprite = (id: number) => {
    const root = new Container();
    const size = TILE * 0.8;
    const body = new Graphics().roundRect(-size / 2, -size / 2, size, size, 6).fill(PLAYER_COLORS[id]);
    if (id === client.you) body.stroke({ width: 3, color: 0xffffff });
    const name = new Text({
      text: client.playerName(id),
      style: { fontFamily: 'system-ui, sans-serif', fontSize: 11, fill: 0xffffff, fontWeight: '600' },
      resolution: 3,
    });
    name.anchor.set(0.5, 1);
    name.y = -TILE * 0.45;
    root.addChild(body, name);
    world.addChild(root);
    world.addChild(bombLayer); // keep bombs on top of newly added players
    const sprite = { root, name };
    sprites.set(id, sprite);
    return sprite;
  };

  const fitToScreen = () => {
    const w = client.mapWidth * TILE;
    const h = client.mapHeight * TILE;
    if (!w) return;
    const margin = 8;
    const availW = app.screen.width - insets.left - insets.right - margin * 2;
    const availH = app.screen.height - insets.top - margin * 2;
    const scale = Math.max(0.1, Math.min(availW / w, availH / h));
    world.scale.set(scale);
    world.position.set(
      insets.left + margin + (availW - w * scale) / 2,
      insets.top + margin + (availH - h * scale) / 2,
    );
  };

  app.ticker.add(() => {
    if (client.mapVersion !== drawnMap) {
      drawMap();
      drawnMap = client.mapVersion;
    }
    if (client.rosterVersion !== drawnRoster) {
      for (const [id, s] of sprites) s.name.text = client.playerName(id);
      drawnRoster = client.rosterVersion;
    }
    fitToScreen();
    drawEffects();

    // Flat [id, x, y, alive, bombs, range, speedUps, ...] from WASM — one copy per frame
    // instead of one call per field.
    // Our own player is predicted, everyone else interpolated — WASM decides.
    const players = client.players(performance.now(), clock.alpha);
    const seen = new Set<number>();
    for (let i = 0; i < players.length; i += 7) {
      const [id, x, y, alive] = [players[i], players[i + 1], players[i + 2], players[i + 3]];
      seen.add(id);
      const s = sprites.get(id) ?? makeSprite(id);
      s.root.position.set(x * unitToPx, y * unitToPx);
      // Dead players stay visible as ghosts until the next round.
      s.root.alpha = alive === 1 ? 1 : 0.25;
    }
    for (const [id, s] of sprites) {
      if (!seen.has(id)) {
        s.root.destroy({ children: true });
        sprites.delete(id);
      }
    }
  });
}
