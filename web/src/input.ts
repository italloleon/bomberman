// Keyboard state, read once per game tick. Direction codes match the WASM `Client.tick`:
// -1 none, 0 up, 1 down, 2 left, 3 right.

const KEY_DIRS: Record<string, number> = {
  ArrowUp: 0,
  KeyW: 0,
  ArrowDown: 1,
  KeyS: 1,
  ArrowLeft: 2,
  KeyA: 2,
  ArrowRight: 3,
  KeyD: 3,
};

export interface Controls {
  /** Direction currently held (most recently pressed wins), or -1. */
  dir(): number;
  /** True once per bomb key press: the press is kept until a tick consumes it, so a
   *  tap shorter than a tick still counts. */
  takeBomb(): boolean;
}

/** Several input sources at once (keyboard + touch): any held direction, any press. */
export function combineControls(...sources: Controls[]): Controls {
  return {
    dir: () => sources.map((s) => s.dir()).find((d) => d !== -1) ?? -1,
    // Call every source's takeBomb so none keeps a stale press for the next tick.
    takeBomb: () => sources.map((s) => s.takeBomb()).some(Boolean),
  };
}

export function keyboardControls(): Controls {
  // Directions currently held, in press order. The most recent one wins, so pressing
  // Right while holding Up turns right, and releasing Right goes back to Up.
  const held: number[] = [];
  let bombPressed = false;

  window.addEventListener('keydown', (e) => {
    if (e.target instanceof HTMLInputElement) return;
    const dir = KEY_DIRS[e.code];
    if (dir !== undefined) {
      e.preventDefault();
      if (!held.includes(dir)) held.push(dir);
    } else if (e.code === 'Space') {
      e.preventDefault();
      if (!e.repeat) bombPressed = true;
    }
  });

  window.addEventListener('keyup', (e) => {
    const dir = KEY_DIRS[e.code];
    if (dir !== undefined) held.splice(held.indexOf(dir), 1);
  });

  // Alt-tabbing away while holding a key would otherwise leave the player walking forever.
  window.addEventListener('blur', () => {
    held.length = 0;
  });

  return {
    dir: () => (held.length ? held[held.length - 1] : -1),
    takeBomb: () => {
      const pressed = bombPressed;
      bombPressed = false;
      return pressed;
    },
  };
}
