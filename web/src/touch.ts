// On-screen controls for phones/tablets: a D-pad on the left, a bomb button on the right.
// Uses Pointer Events so both thumbs work at once and each control tracks its own finger.

import type { Controls } from './input';

/** Touch devices get the on-screen controls; `?touch=1` forces them on (for testing). */
export function wantsTouchControls(): boolean {
  return (
    new URLSearchParams(location.search).has('touch') ||
    window.matchMedia('(pointer: coarse)').matches
  );
}

/** Fraction of the pad's radius that counts as "no direction". */
const DEAD_ZONE = 0.18;

const ARROW = 'M50 14 L64 34 L36 34 Z'; // up-pointing triangle; rotated for the others

export function touchControls(): Controls {
  document.body.classList.add('touch');

  const pad = document.createElement('div');
  pad.className = 'dpad';
  // One arrow per direction, in Controls' dir order: up, down, left, right.
  pad.innerHTML = `
    <svg viewBox="0 0 100 100" aria-hidden="true">
      <circle cx="50" cy="50" r="47" class="base" />
      ${[0, 180, 270, 90].map((deg, dir) => `<path d="${ARROW}" data-dir="${dir}" transform="rotate(${deg} 50 50)" />`).join('')}
      <circle cx="50" cy="50" r="9" class="hub" />
    </svg>`;

  const bombButton = document.createElement('button');
  bombButton.className = 'bomb-button';
  bombButton.setAttribute('aria-label', 'Drop bomb');
  bombButton.textContent = '💣';

  document.body.append(pad, bombButton);

  let dir = -1;
  let padPointer: number | null = null;
  let bombPressed = false;

  const setDir = (d: number) => {
    if (d === dir) return;
    dir = d;
    pad.querySelectorAll('path').forEach((p) => p.classList.toggle('active', Number(p.dataset.dir) === d));
  };

  // Direction = the dominant axis of the finger's offset from the pad's centre.
  const aim = (e: PointerEvent) => {
    const r = pad.getBoundingClientRect();
    const dx = e.clientX - (r.left + r.width / 2);
    const dy = e.clientY - (r.top + r.height / 2);
    if (Math.hypot(dx, dy) < (r.width / 2) * DEAD_ZONE) setDir(-1);
    else if (Math.abs(dx) > Math.abs(dy)) setDir(dx > 0 ? 3 : 2);
    else setDir(dy > 0 ? 1 : 0);
  };

  pad.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    padPointer = e.pointerId;
    // Keep receiving this finger's moves even if it slides off the pad.
    pad.setPointerCapture(e.pointerId);
    aim(e);
  });
  pad.addEventListener('pointermove', (e) => {
    if (e.pointerId === padPointer) aim(e);
  });
  const release = (e: PointerEvent) => {
    if (e.pointerId !== padPointer) return;
    padPointer = null;
    setDir(-1);
  };
  pad.addEventListener('pointerup', release);
  pad.addEventListener('pointercancel', release);

  bombButton.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    bombPressed = true;
    bombButton.classList.add('pressed');
    navigator.vibrate?.(15);
  });
  const unpress = () => bombButton.classList.remove('pressed');
  bombButton.addEventListener('pointerup', unpress);
  bombButton.addEventListener('pointercancel', unpress);

  return {
    dir: () => dir,
    takeBomb: () => {
      const pressed = bombPressed;
      bombPressed = false;
      return pressed;
    },
  };
}

/** Best effort: fullscreen hides the browser bars, and lets Android lock landscape.
 *  Must run inside a user gesture (e.g. the Join tap). iPhone Safari supports neither. */
export async function enterMobileFullscreen(): Promise<void> {
  try {
    await document.documentElement.requestFullscreen?.({ navigationUI: 'hide' });
    // Not in every TS lib version yet, hence the cast.
    await (screen.orientation as ScreenOrientation & { lock?: (o: string) => Promise<void> }).lock?.('landscape');
  } catch {
    // Unsupported or denied: the rotate-your-phone overlay covers portrait anyway.
  }
}
