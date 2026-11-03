<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { paintHolo, type HoloScene } from "../holo";

const props = defineProps<{ scene?: HoloScene | null }>();
const host = ref<HTMLElement | null>(null);
let renderer: THREE.WebGLRenderer | null = null;
let world: THREE.Scene | null = null;
let camera: THREE.PerspectiveCamera | null = null;
let controls: OrbitControls | null = null;
let frame = 0;

function draw() {
  if (world && props.scene) paintHolo(world, props.scene);
}

function loop() {
  frame = requestAnimationFrame(loop);
  controls?.update();
  if (renderer && world && camera) renderer.render(world, camera);
}

function fit() {
  if (!host.value || !renderer || !camera) return;
  const width = host.value.clientWidth;
  const height = Math.max(host.value.clientHeight, 1);
  const view = camera as unknown as { aspect: number; updateProjectionMatrix: () => void };
  view.aspect = width / height;
  view.updateProjectionMatrix();
  renderer.setSize(width, height);
}

onMounted(() => {
  if (!host.value) return;
  world = new THREE.Scene();
  world.background = new THREE.Color(0x070b10);
  camera = new THREE.PerspectiveCamera(50, 1, 0.1, 100);
  camera.position.set(0, 2.2, 8);
  renderer = new THREE.WebGLRenderer({ antialias: true });
  host.value.appendChild(renderer.domElement);
  controls = new OrbitControls(camera, renderer.domElement);
  controls.enablePan = false;
  fit();
  draw();
  loop();
  window.addEventListener("resize", fit);
});

watch(() => props.scene, draw);

onBeforeUnmount(() => {
  cancelAnimationFrame(frame);
  window.removeEventListener("resize", fit);
  controls?.dispose();
  renderer?.dispose();
});
</script>

<template>
  <div ref="host" class="holo-host"></div>
</template>
