<script setup lang="ts">
/**
 * Big Picture on a handheld: a drawn device whose screen walks through Irori's
 * Big Picture themes and layouts. Its shoulder buttons and d-pad step through
 * them, as a controller would; the list below picks one directly. While it is
 * in view it moves on by itself until the visitor takes over, and never with
 * reduced motion. The light behind the device takes each screen's colour.
 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'

export interface HandheldShot {
  /** File stem under /screenshots/bigpicture/. */
  stem: string
  theme: string
  layout: string
  /** The screen's most telling colour, for the light behind the device. */
  accent: string
}

const props = defineProps<{
  shots: HandheldShot[]
  labels: { list: string; previous: string; next: string; shot: string; pause: string; play: string }
}>()

const DWELL_MS = 5200

const index = ref(0)
const pressed = ref<'left' | 'right' | null>(null)
const playing = ref(false)
const tilt = ref({ x: 0, y: 0 })
const stage = ref<HTMLElement | null>(null)
let timer: number | undefined
let pressTimer: number | undefined
let observer: IntersectionObserver | undefined
let reduced = false
let inView = false
let held = false

const current = computed(() => props.shots[index.value]!)
const describe = (shot: HandheldShot) => props.labels.shot.replace('{theme}', shot.theme).replace('{layout}', shot.layout)

function show(next: number, from?: 'left' | 'right') {
  const count = props.shots.length
  index.value = (next + count) % count
  if (from) {
    pressed.value = from
    window.clearTimeout(pressTimer)
    pressTimer = window.setTimeout(() => (pressed.value = null), 180)
  }
  schedule()
}

/** The visitor chose: stop moving on by itself. */
function choose(next: number, from?: 'left' | 'right') {
  playing.value = false
  show(next, from)
}

function schedule() {
  window.clearTimeout(timer)
  if (!playing.value || !inView || held || reduced) return
  timer = window.setTimeout(() => show(index.value + 1, 'right'), DWELL_MS)
}

function toggle() {
  playing.value = !playing.value
  schedule()
}

function onPointerMove(event: PointerEvent) {
  if (reduced || event.pointerType !== 'mouse' || !stage.value) return
  const box = stage.value.getBoundingClientRect()
  const x = (event.clientX - box.left) / box.width - 0.5
  const y = (event.clientY - box.top) / box.height - 0.5
  tilt.value = { x: -y * 7, y: x * 9 }
}

function hold(on: boolean) {
  held = on
  if (!on) tilt.value = { x: 0, y: 0 }
  schedule()
}

function onKey(event: KeyboardEvent) {
  if (event.key === 'ArrowRight') choose(index.value + 1, 'right')
  else if (event.key === 'ArrowLeft') choose(index.value - 1, 'left')
  else return
  event.preventDefault()
}

onMounted(() => {
  reduced = matchMedia('(prefers-reduced-motion: reduce)').matches
  playing.value = !reduced
  if (!stage.value || !('IntersectionObserver' in window)) return
  observer = new IntersectionObserver(([entry]) => {
    inView = !!entry?.isIntersecting
    schedule()
  }, { threshold: 0.35 })
  observer.observe(stage.value)
})

onBeforeUnmount(() => {
  observer?.disconnect()
  window.clearTimeout(timer)
  window.clearTimeout(pressTimer)
})
</script>

<template>
  <div class="flex flex-col items-center gap-8">
    <figure
      ref="stage"
      class="stage"
      :style="{ '--accent': current.accent }"
      @pointermove="onPointerMove"
      @pointerenter="hold(true)"
      @pointerleave="hold(false)"
      @focusin="hold(true)"
      @focusout="hold(false)"
    >
      <div class="aura" aria-hidden="true"></div>
      <div class="device" :style="{ transform: `rotateX(${tilt.x}deg) rotateY(${tilt.y}deg)` }">
        <button type="button" class="bumper left" :class="{ down: pressed === 'left' }" :aria-label="labels.previous" @click="choose(index - 1, 'left')">
          <span aria-hidden="true">L</span>
        </button>
        <button type="button" class="bumper right" :class="{ down: pressed === 'right' }" :aria-label="labels.next" @click="choose(index + 1, 'right')">
          <span aria-hidden="true">R</span>
        </button>

        <div class="shell">
          <div class="grip">
            <span class="stick" aria-hidden="true"><span></span></span>
            <span class="dpad" aria-hidden="true">
              <span class="arm up"></span>
              <span class="arm down"></span>
              <span class="arm left" :class="{ down: pressed === 'left' }"></span>
              <span class="arm right" :class="{ down: pressed === 'right' }"></span>
            </span>
            <span class="key small" aria-hidden="true"></span>
          </div>

          <div class="bezel" tabindex="0" :aria-label="labels.list" @keydown="onKey">
            <div class="screen">
              <img
                v-for="(shot, i) in shots"
                :key="shot.stem"
                :src="`/screenshots/bigpicture/${shot.stem}.webp`"
                :srcset="`/screenshots/bigpicture/${shot.stem}.webp 1280w, /screenshots/bigpicture/${shot.stem}@2x.webp 2560w`"
                sizes="(min-width: 1280px) 820px, 72vw"
                :alt="i === index ? `Irori Big Picture: ${describe(shot)}` : ''"
                :aria-hidden="i !== index"
                width="1280"
                height="800"
                :loading="i === 0 ? 'eager' : 'lazy'"
                decoding="async"
                class="frame"
                :class="{ on: i === index }"
              />
              <span class="glass" aria-hidden="true"></span>
            </div>
          </div>

          <div class="grip">
            <span class="face" aria-hidden="true">
              <span class="key-face y">Y</span>
              <span class="key-face x">X</span>
              <span class="key-face b">B</span>
              <span class="key-face a" :class="{ down: pressed === 'right' }">A</span>
            </span>
            <span class="stick" aria-hidden="true"><span></span></span>
            <span class="key small" aria-hidden="true"></span>
          </div>
        </div>
        <span class="led" aria-hidden="true"></span>
      </div>
      <figcaption class="sr-only" aria-live="polite">{{ describe(current) }}</figcaption>
    </figure>

    <div class="flex w-full max-w-6xl flex-col items-center gap-4">
      <ul class="flex flex-wrap justify-center gap-1.5" :aria-label="labels.list">
        <li v-for="(shot, i) in shots" :key="shot.stem">
          <button
            type="button"
            class="chip"
            :class="{ on: i === index, timed: i === index && playing }"
            :style="{ '--chip': shot.accent, '--dwell': `${DWELL_MS}ms` }"
            :aria-pressed="i === index"
            @click="choose(i)"
          >
            <span class="dot" aria-hidden="true"></span>
            <span class="font-medium text-paper">{{ shot.theme }}</span>
            <span class="text-smoke">{{ shot.layout }}</span>
            <span class="bar" aria-hidden="true"></span>
          </button>
        </li>
      </ul>
      <button type="button" class="link text-sm" @click="toggle">{{ playing ? labels.pause : labels.play }}</button>
    </div>
  </div>
</template>

<style scoped>
.stage {
  position: relative;
  width: min(100%, 68rem);
  margin: 0;
  container-type: inline-size;
  perspective: 1600px;
}

/* The screen's light, falling on whatever is behind the device. */
.aura {
  position: absolute;
  inset: 8% 12% -4%;
  border-radius: 50%;
  background: radial-gradient(closest-side, color-mix(in oklab, var(--accent) 70%, transparent), transparent);
  filter: blur(56px);
  opacity: 0.55;
  transition: background 900ms var(--ease-out);
}

.device {
  position: relative;
  transform-style: preserve-3d;
  transition: transform 500ms var(--ease-out);
  padding-top: 2.8cqw;
}

.shell {
  position: relative;
  display: grid;
  grid-template-columns: 15% minmax(0, 1fr) 15%;
  align-items: center;
  gap: 1.2cqw;
  padding: 2.1cqw 2.2cqw;
  border-radius: 6cqw / 9cqw;
  background:
    radial-gradient(120% 90% at 50% -20%, rgb(255 255 255 / 0.09), transparent 55%),
    linear-gradient(180deg, #2a2420 0%, #1b1714 55%, #120f0d 100%);
  box-shadow:
    inset 0 1px 0 rgb(255 255 255 / 0.12),
    inset 0 -2px 0 rgb(0 0 0 / 0.5),
    inset 0 0 0 1px rgb(255 255 255 / 0.04),
    0 50px 90px -40px rgb(0 0 0 / 0.75),
    0 24px 40px -28px rgb(0 0 0 / 0.6);
}

/* A seam of warm wood across the back, the site's hearth in the hand. */
.shell::after {
  content: '';
  position: absolute;
  inset: auto 18% -0.45cqw;
  height: 0.9cqw;
  border-radius: 999px;
  background: linear-gradient(90deg, transparent, var(--ember), var(--flame), var(--ember), transparent);
  opacity: 0.55;
  filter: blur(0.4cqw);
}

.grip {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: space-between;
  gap: 2.4cqw;
  height: 100%;
  padding: 1.4cqw 0;
}

.bezel {
  position: relative;
  padding: 1.1cqw;
  border-radius: 1.8cqw;
  background: #070605;
  box-shadow: inset 0 0 0 1px rgb(255 255 255 / 0.05), inset 0 2px 8px rgb(0 0 0 / 0.8);
  outline: none;
}
.bezel:focus-visible {
  box-shadow: inset 0 0 0 1px rgb(255 255 255 / 0.05), 0 0 0 3px var(--ember);
}

.screen {
  position: relative;
  aspect-ratio: 16 / 10;
  overflow: hidden;
  border-radius: 0.9cqw;
  background: #000;
}

.frame {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
  opacity: 0;
  transform: scale(1.015);
  transition: opacity 700ms var(--ease-out), transform 1200ms var(--ease-out);
}
.frame.on {
  opacity: 1;
  transform: none;
}

/* Glass: a soft sheen across the top corner. */
.glass {
  position: absolute;
  inset: 0;
  pointer-events: none;
  background: linear-gradient(118deg, rgb(255 255 255 / 0.1) 0%, rgb(255 255 255 / 0.03) 22%, transparent 38%);
}

.stick {
  display: grid;
  place-items: center;
  width: 7.2cqw;
  aspect-ratio: 1;
  border-radius: 50%;
  background: radial-gradient(circle at 50% 40%, #2e2824, #0f0c0b 70%);
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.08), 0 0.5cqw 1cqw rgb(0 0 0 / 0.55);
}
.stick > span {
  width: 64%;
  aspect-ratio: 1;
  border-radius: 50%;
  background:
    repeating-radial-gradient(circle at 50% 50%, rgb(255 255 255 / 0.05) 0 1px, transparent 1px 4px),
    radial-gradient(circle at 45% 35%, #4a413a, #231d19);
  box-shadow: inset 0 -2px 3px rgb(0 0 0 / 0.5), 0 1px 0 rgb(255 255 255 / 0.06);
}

.dpad {
  position: relative;
  display: block;
  width: 7.4cqw;
  aspect-ratio: 1;
}
.arm {
  position: absolute;
  background: linear-gradient(180deg, #312a25, #1a1512);
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.08), 0 0.3cqw 0.6cqw rgb(0 0 0 / 0.5);
  transition: background 160ms, box-shadow 160ms, transform 160ms;
}
.arm.up,
.arm.down {
  left: 33%;
  width: 34%;
  height: 38%;
}
.arm.up {
  top: 0;
  border-radius: 0.5cqw 0.5cqw 0.15cqw 0.15cqw;
}
.arm.down {
  bottom: 0;
  border-radius: 0.15cqw 0.15cqw 0.5cqw 0.5cqw;
}
.arm.left,
.arm.right {
  top: 33%;
  height: 34%;
  width: 38%;
}
.arm.left {
  left: 0;
  border-radius: 0.5cqw 0.15cqw 0.15cqw 0.5cqw;
}
.arm.right {
  right: 0;
  border-radius: 0.15cqw 0.5cqw 0.5cqw 0.15cqw;
}
.arm.down,
.arm.left.down,
.arm.right.down {
  transform: none;
}
.arm.left.down,
.arm.right.down {
  background: linear-gradient(180deg, var(--ember), var(--flame));
  box-shadow: 0 0 1.4cqw var(--flame);
  transform: scale(0.94);
}

.face {
  position: relative;
  display: block;
  width: 8.4cqw;
  aspect-ratio: 1;
}
.key-face {
  position: absolute;
  display: grid;
  place-items: center;
  width: 34%;
  aspect-ratio: 1;
  border-radius: 50%;
  font: 600 1.15cqw/1 var(--font-code);
  color: rgb(242 233 220 / 0.55);
  background: radial-gradient(circle at 45% 35%, #3a322c, #17120f);
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.1), 0 0.35cqw 0.7cqw rgb(0 0 0 / 0.55);
  transition: background 160ms, color 160ms, box-shadow 160ms, transform 160ms;
}
.key-face.y {
  top: 0;
  left: 33%;
}
.key-face.x {
  top: 33%;
  left: 0;
}
.key-face.b {
  top: 33%;
  right: 0;
}
.key-face.a {
  bottom: 0;
  left: 33%;
}
.key-face.a.down {
  color: var(--flame-ink);
  background: radial-gradient(circle at 45% 35%, var(--ember), var(--flame));
  box-shadow: 0 0 1.6cqw var(--flame);
  transform: scale(0.92);
}

.key.small {
  display: block;
  width: 2.2cqw;
  height: 0.9cqw;
  border-radius: 999px;
  background: #0f0c0b;
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.07);
}

.bumper {
  position: absolute;
  top: 0;
  width: 17%;
  height: 4.6cqw;
  border-radius: 3cqw 3cqw 0.8cqw 0.8cqw;
  background: linear-gradient(180deg, #3a322c, #1c1714);
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.1);
  color: rgb(242 233 220 / 0.5);
  font: 600 1cqw/1 var(--font-code);
  cursor: pointer;
  transition: transform 160ms var(--ease-out), background 160ms;
}
.bumper span {
  position: absolute;
  top: 0.9cqw;
}
.bumper.left {
  left: 7%;
}
.bumper.left span {
  left: 2.2cqw;
}
.bumper.right {
  right: 7%;
}
.bumper.right span {
  right: 2.2cqw;
}
.bumper:hover,
.bumper:focus-visible {
  transform: translateY(-0.35cqw);
  color: var(--paper);
}
.bumper.down {
  transform: translateY(0.25cqw);
  background: linear-gradient(180deg, var(--ember), var(--flame));
  color: var(--flame-ink);
}
.bumper:focus-visible {
  outline: 3px solid var(--ember);
  outline-offset: 2px;
}

/* The power light, breathing slowly. */
.led {
  position: absolute;
  right: 9%;
  bottom: 3.2cqw;
  width: 0.7cqw;
  aspect-ratio: 1;
  border-radius: 50%;
  background: var(--flame);
  box-shadow: 0 0 1cqw var(--flame);
}

.chip {
  position: relative;
  display: inline-flex;
  align-items: center;
  gap: 0.5rem;
  overflow: hidden;
  padding: 0.45rem 0.8rem;
  border: 1px solid var(--grain);
  border-radius: 999px;
  background: var(--soot);
  font-size: 0.88rem;
  transition: border-color 200ms, background 200ms, transform 200ms var(--ease-out);
}
.chip:hover {
  transform: translateY(-1px);
}
.chip.on {
  border-color: color-mix(in oklab, var(--chip) 60%, var(--grain));
  background: color-mix(in oklab, var(--chip) 14%, var(--soot));
}
.chip:focus-visible {
  outline: 3px solid var(--ember);
  outline-offset: 2px;
}
.dot {
  width: 0.55rem;
  aspect-ratio: 1;
  border-radius: 50%;
  background: var(--chip);
  box-shadow: 0 0 0.6rem var(--chip);
}
/* How long until the next screen. */
.bar {
  position: absolute;
  left: 0;
  bottom: 0;
  height: 2px;
  width: 100%;
  background: var(--chip);
  transform-origin: left;
  transform: scaleX(0);
}
.chip.timed .bar {
  animation: dwell var(--dwell) linear forwards;
}
@keyframes dwell {
  to {
    transform: scaleX(1);
  }
}

/* On a phone the screen takes more of the device. */
@container (max-width: 560px) {
  .shell {
    grid-template-columns: 12% minmax(0, 1fr) 12%;
    gap: 0.8cqw;
    padding: 1.8cqw 1.6cqw;
  }
}

@media (prefers-reduced-motion: reduce) {
  .device,
  .frame,
  .aura,
  .arm,
  .key-face,
  .bumper,
  .chip {
    transition: none;
  }
  .frame {
    transform: none;
  }
}

@media (prefers-reduced-motion: no-preference) {
  .led {
    animation: breathe 3.2s ease-in-out infinite;
  }
  @keyframes breathe {
    50% {
      opacity: 0.45;
      box-shadow: 0 0 0.4cqw var(--flame);
    }
  }
}
</style>
