declare module "three" {
  export class Color {
    constructor(hex: number);
  }
  export class Vector3 {
    constructor(x?: number, y?: number, z?: number);
    x: number;
    y: number;
    z: number;
    copy(other: Vector3): this;
    set(x: number, y: number, z: number): this;
  }
  export class Scene {
    background: Color | null;
    add(object: object): this;
    clear(): this;
  }
  export class PerspectiveCamera {
    constructor(fov: number, aspect: number, near: number, far: number);
    position: Vector3;
  }
  export class WebGLRenderer {
    constructor(options?: { antialias?: boolean });
    domElement: HTMLCanvasElement;
    setSize(width: number, height: number): void;
    render(scene: Scene, camera: PerspectiveCamera): void;
    dispose(): void;
  }
  export class AmbientLight {
    constructor(color: number, intensity?: number);
  }
  export class SphereGeometry {
    constructor(radius: number, widthSegments: number, heightSegments: number);
  }
  export class MeshBasicMaterial {
    constructor(parameters?: object);
  }
  export class Mesh {
    constructor(geometry: SphereGeometry, material: MeshBasicMaterial);
    position: Vector3;
  }
  export class CanvasTexture {
    constructor(canvas: HTMLCanvasElement);
  }
  export class SpriteMaterial {
    constructor(parameters?: object);
  }
  export class Sprite {
    constructor(material?: SpriteMaterial);
    position: Vector3;
    scale: Vector3;
  }
  export class BufferGeometry {
    setAttribute(name: string, attribute: Float32BufferAttribute): this;
  }
  export class Float32BufferAttribute {
    constructor(array: number[], itemSize: number);
  }
  export class LineBasicMaterial {
    constructor(parameters?: object);
  }
  export class LineSegments {
    constructor(geometry: BufferGeometry, material: LineBasicMaterial);
  }
}

declare module "three/addons/controls/OrbitControls.js" {
  import { PerspectiveCamera } from "three";
  export class OrbitControls {
    constructor(camera: PerspectiveCamera, domElement: HTMLElement);
    enablePan: boolean;
    update(): void;
    dispose(): void;
  }
}
