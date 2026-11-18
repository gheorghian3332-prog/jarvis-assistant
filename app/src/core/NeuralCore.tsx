import { Component, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import { Canvas, useFrame, useThree } from "@react-three/fiber";
import {
  AdditiveBlending,
  BufferAttribute,
  BufferGeometry,
  Color,
  Group,
  LinearSRGBColorSpace,
  LineSegments,
  Mesh,
  Points,
  ShaderMaterial,
  Vector3,
  type IUniform,
  type PerspectiveCamera,
} from "three";
import { BRAIN_CENTER_Y, BRAIN_RADIUS, ELEVATION, FOV_DEG, START_AZIMUTH, fitDistance } from "./camera";
import type { AssistantState } from "../api";
import {
  adaptiveConfigFor,
  createAdaptiveState,
  dprFor,
  estimateRefreshMs,
  maxLevelFor,
  powerPreferenceFor,
  QUALITY_SPECS,
  resolveQuality,
  stepAdaptive,
  type AdaptiveConfig,
  type AdaptiveState,
  type QualityLevel,
  type QualitySetting,
} from "./quality";
import { browserLoopEnv, createFrameLoop, targetFps, type FpsLimit } from "./fps";
import { smoothLevel } from "./level";
import { getModelData, type ModelData } from "./model";
import {
  approach,
  errorFlash,
  paramsForState,
  type CoreParams,
} from "./states";
import {
  GLOW_FRAGMENT,
  GLOW_VERTEX,
  LINE_FRAGMENT,
  LINE_VERTEX,
  NEURON_FRAGMENT,
  NEURON_VERTEX,
  PULSE_FRAGMENT,
  PULSE_VERTEX,
} from "./shaders";

export interface NeuralCoreProps {
  state: AssistantState;
  /** Latest `assistant://level` rms (or the preview slider); read every frame, never causes a render. */
  levelRef: RefObject<number>;
  quality: QualitySetting;
  fpsLimit: FpsLimit;
  reducedMotion: boolean;
  compact: boolean;
  debug: boolean;
  unavailableText: string;
}

const FOV = FOV_DEG;
const CAMERA_Z = 4.4;
const CENTER_SHIFT_Y = -BRAIN_CENTER_Y;
/** Page background (#080c10) written as raw values: the renderer runs without colour conversion. */
const BACKGROUND = new Color().setRGB(8 / 255, 12 / 255, 16 / 255, LinearSRGBColorSpace);
const MAX_DT = 0.1;
const START_ANGLE = START_AZIMUTH;
/** Frame intervals are ignored for this long after mount: shader compilation and layout settle. */
const WARMUP_MS = 2000;
const REFRESH_SAMPLES = 90;
const OVERLAY_PERIOD_MS = 500;

interface Stats {
  neurons: number;
  edges: number;
  pulses: number;
  level: QualityLevel;
}

const NUMERIC_UNIFORMS = [
  "u_time",
  "u_pulseClock",
  "u_wavePhase",
  "u_viewScale",
  "u_sizeScale",
  "u_camDist",
  "u_expand",
  "u_level",
  "u_jitter",
  "u_waveAmt",
  "u_waveDir",
  "u_flicker",
  "u_throb",
  "u_flash",
  "u_intensity",
  "u_density",
] as const;

type NumericUniform = (typeof NUMERIC_UNIFORMS)[number];

interface CoreUniforms {
  numeric: Record<NumericUniform, IUniform<number>>;
  color: IUniform<Vector3>;
  /** The object handed to every ShaderMaterial (same IUniform instances, one set of values). */
  all: Record<string, IUniform>;
}

function createUniforms(): CoreUniforms {
  const initial: Record<NumericUniform, number> = {
    u_time: 0,
    u_pulseClock: 0,
    u_wavePhase: 0,
    u_viewScale: 600,
    u_sizeScale: QUALITY_SPECS.medium.sizeScale,
    u_camDist: CAMERA_Z,
    u_expand: 0,
    u_level: 0,
    u_jitter: 0,
    u_waveAmt: 0,
    u_waveDir: 0,
    u_flicker: 0,
    u_throb: 1,
    u_flash: 0,
    u_intensity: 0.8,
    u_density: 0,
  };
  const numeric = Object.fromEntries(NUMERIC_UNIFORMS.map((name) => [name, { value: initial[name] }])) as Record<
    NumericUniform,
    IUniform<number>
  >;
  const color: IUniform<Vector3> = { value: new Vector3(0.1, 0.78, 0.86) };
  return { numeric, color, all: { ...numeric, u_color: color } };
}

function makeMaterial(uniforms: Record<string, IUniform>, vertexShader: string, fragmentShader: string): ShaderMaterial {
  return new ShaderMaterial({
    uniforms,
    vertexShader,
    fragmentShader,
    transparent: true,
    depthWrite: false,
    depthTest: false,
    blending: AdditiveBlending,
  });
}

function attribute(array: Float32Array, itemSize: number): BufferAttribute {
  return new BufferAttribute(array, itemSize);
}

function buildGroup(
  data: ModelData,
  materials: { neurons: ShaderMaterial; lines: ShaderMaterial; pulses: ShaderMaterial; glow: ShaderMaterial },
): { group: Group; geometries: BufferGeometry[] } {
  const neuronGeometry = new BufferGeometry();
  neuronGeometry.setAttribute("position", attribute(data.neurons.positions, 3));
  neuronGeometry.setAttribute("a_size", attribute(data.neurons.sizes, 1));
  neuronGeometry.setAttribute("a_phase", attribute(data.neurons.phases, 1));
  neuronGeometry.setAttribute("a_bright", attribute(data.neurons.brights, 1));

  const lineGeometry = new BufferGeometry();
  lineGeometry.setAttribute("position", attribute(data.linePositions, 3));
  lineGeometry.setAttribute("a_hub", attribute(data.lineHubs, 1));

  const tails = data.pulseTails;
  const pulseGeometry = new BufferGeometry();
  pulseGeometry.setAttribute("position", attribute(tails.starts, 3));
  pulseGeometry.setAttribute("a_start", attribute(tails.starts, 3));
  pulseGeometry.setAttribute("a_end", attribute(tails.ends, 3));
  pulseGeometry.setAttribute("a_offset", attribute(tails.offsets, 1));
  pulseGeometry.setAttribute("a_speed", attribute(tails.speeds, 1));
  pulseGeometry.setAttribute("a_tail", attribute(tails.tails, 1));

  const glowGeometry = new BufferGeometry();
  glowGeometry.setAttribute("position", attribute(new Float32Array([-1, -1, 0, 1, -1, 0, 1, 1, 0, -1, 1, 0]), 3));
  glowGeometry.setIndex([0, 1, 2, 0, 2, 3]);

  const glow = new Mesh(glowGeometry, materials.glow);
  const lines = new LineSegments(lineGeometry, materials.lines);
  const pulses = new Points(pulseGeometry, materials.pulses);
  const neurons = new Points(neuronGeometry, materials.neurons);
  const ordered = [glow, lines, pulses, neurons];
  ordered.forEach((object, order) => {
    object.frustumCulled = false;
    object.renderOrder = order;
  });
  const group = new Group();
  group.add(...ordered);
  return { group, geometries: [neuronGeometry, lineGeometry, pulseGeometry, glowGeometry] };
}

interface BrainProps {
  data: ModelData;
  state: AssistantState;
  levelRef: RefObject<number>;
  reducedMotion: boolean;
  statsRef: RefObject<Stats>;
}

interface Runtime {
  params: CoreParams;
  level: number;
  time: number;
  pulseClock: number;
  wavePhase: number;
  angle: number;
  state: AssistantState;
  enteredAt: number;
}

/** The scene: three draw calls, animated only through uniforms and the group's rotation. */
function Brain({ data, state, levelRef, reducedMotion, statsRef }: BrainProps) {
  const uniforms = useMemo(createUniforms, []);
  const materials = useMemo(
    () => ({
      neurons: makeMaterial(uniforms.all, NEURON_VERTEX, NEURON_FRAGMENT),
      lines: makeMaterial(uniforms.all, LINE_VERTEX, LINE_FRAGMENT),
      pulses: makeMaterial(uniforms.all, PULSE_VERTEX, PULSE_FRAGMENT),
      glow: makeMaterial(uniforms.all, GLOW_VERTEX, GLOW_FRAGMENT),
    }),
    [uniforms],
  );
  useEffect(
    () => () => {
      materials.neurons.dispose();
      materials.lines.dispose();
      materials.pulses.dispose();
      materials.glow.dispose();
    },
    [materials],
  );
  const built = useMemo(() => buildGroup(data, materials), [data, materials]);
  useEffect(
    () => () => {
      for (const geometry of built.geometries) geometry.dispose();
    },
    [built],
  );

  const runtime = useRef<Runtime>({
    params: paramsForState(state, reducedMotion),
    level: 0,
    time: 0,
    pulseClock: 0,
    wavePhase: 0,
    angle: START_ANGLE,
    state,
    enteredAt: 0,
  });
  const stateRef = useRef(state);
  const reducedRef = useRef(reducedMotion);
  useEffect(() => {
    stateRef.current = state;
    reducedRef.current = reducedMotion;
  }, [state, reducedMotion]);

  useEffect(() => {
    statsRef.current = {
      neurons: data.neurons.count,
      edges: data.edgeCount,
      pulses: data.pulses.count,
      level: data.level,
    };
    uniforms.numeric.u_sizeScale.value = QUALITY_SPECS[data.level].sizeScale;
  }, [data, statsRef, uniforms]);

  const size = useThree((store) => store.size);
  const camera = useThree((store) => store.camera);
  const invalidate = useThree((store) => store.invalidate);
  useLayoutEffect(() => {
    const aspect = size.height > 0 ? size.width / size.height : 1;
    const distance = fitDistance(BRAIN_RADIUS, FOV, aspect);
    const perspective = camera as PerspectiveCamera;
    perspective.position.z = distance;
    perspective.updateProjectionMatrix();
    uniforms.numeric.u_camDist.value = distance;
    invalidate();
  }, [size.width, size.height, camera, invalidate, uniforms]);

  useFrame((frame, delta) => {
    const dt = Math.min(delta, MAX_DT);
    const run = runtime.current;
    run.time += dt;
    const current = stateRef.current;
    if (current !== run.state) {
      run.state = current;
      run.enteredAt = run.time;
    }
    run.params = approach(run.params, paramsForState(current, reducedRef.current), dt);
    run.level = smoothLevel(run.level, levelRef.current, dt * 1000);
    const params = run.params;
    run.pulseClock += dt * params.pulseRate;
    run.wavePhase += dt * params.waveSpeed;

    const u = uniforms.numeric;
    u.u_time.value = run.time;
    u.u_pulseClock.value = run.pulseClock;
    u.u_wavePhase.value = run.wavePhase;
    u.u_viewScale.value = (frame.size.height * frame.viewport.dpr) / (2 * Math.tan((FOV * Math.PI) / 360));
    u.u_expand.value = params.expand + params.breath * Math.sin(run.time * 1.5);
    u.u_level.value = run.level * params.levelGain;
    u.u_jitter.value = params.jitter;
    u.u_waveAmt.value = params.waveAmt;
    u.u_waveDir.value = params.waveDir;
    u.u_flicker.value = params.flicker;
    u.u_throb.value = 1 + params.throbAmp * Math.sin(run.time * Math.PI * 2 * params.throbHz);
    u.u_flash.value = errorFlash(current, run.time - run.enteredAt);
    u.u_intensity.value = params.intensity;
    u.u_density.value = params.pulseDensity;
    uniforms.color.value.set(params.color[0], params.color[1], params.color[2]);

    run.angle += params.rotation * dt;
    built.group.rotation.y = run.angle;
    built.group.rotation.x = ELEVATION;
    built.group.position.y = CENTER_SHIFT_Y;
  });

  return <primitive object={built.group} />;
}

interface DriverProps {
  state: AssistantState;
  fpsLimit: FpsLimit;
  quality: QualitySetting;
  compact: boolean;
  autoLevel: QualityLevel;
  onAutoLevel: (level: QualityLevel) => void;
  statsRef: RefObject<Stats>;
  overlayRef: RefObject<HTMLDivElement | null>;
}

/** Own rAF loop: asks R3F for a frame no more often than the FPS limit; stops while hidden. Also feeds the quality controller. */
function Driver({ state, fpsLimit, quality, compact, autoLevel, onAutoLevel, statsRef, overlayRef }: DriverProps) {
  const invalidate = useThree((store) => store.invalidate);
  const live = useRef({ state, fpsLimit, quality, compact, autoLevel, onAutoLevel });
  useEffect(() => {
    live.current = { state, fpsLimit, quality, compact, autoLevel, onAutoLevel };
  }, [state, fpsLimit, quality, compact, autoLevel, onAutoLevel]);

  useEffect(() => {
    let adaptive: AdaptiveState = createAdaptiveState(live.current.autoLevel, 0);
    let config: AdaptiveConfig = adaptiveConfigFor(0);
    let startedAt: number | null = null;
    let lastQuality: QualitySetting = live.current.quality;
    const refreshSamples: number[] = [];
    let emaMs = 0;
    let renders = 0;
    let windowStart = 0;
    let shownFps = 0;

    const loop = createFrameLoop(browserLoopEnv(), {
      getFps: () => targetFps(live.current.state, live.current.fpsLimit),
      onTick: (intervalMs, nowMs) => {
        startedAt ??= nowMs;
        emaMs = emaMs === 0 ? intervalMs : emaMs + (intervalMs - emaMs) * 0.1;
        const current = live.current;
        if (current.quality !== lastQuality) {
          adaptive = createAdaptiveState(current.autoLevel, nowMs);
          lastQuality = current.quality;
        }
        if (nowMs - startedAt >= WARMUP_MS && current.quality === "auto") {
          if (refreshSamples.length < REFRESH_SAMPLES) {
            refreshSamples.push(intervalMs);
            if (refreshSamples.length === REFRESH_SAMPLES) config = adaptiveConfigFor(estimateRefreshMs(refreshSamples));
          }
          const next = stepAdaptive(adaptive, intervalMs, nowMs, maxLevelFor(current.compact), config);
          if (next.level !== adaptive.level) current.onAutoLevel(next.level);
          adaptive = next;
        }
      },
      onRender: (nowMs) => {
        renders += 1;
        invalidate();
        const overlay = overlayRef.current;
        if (windowStart === 0) windowStart = nowMs;
        if (nowMs - windowStart >= OVERLAY_PERIOD_MS) {
          shownFps = (renders * 1000) / (nowMs - windowStart);
          renders = 0;
          windowStart = nowMs;
          const stats = statsRef.current;
          if (overlay && stats) {
            overlay.textContent = [
              `fps ${shownFps.toFixed(1)} / ${targetFps(live.current.state, live.current.fpsLimit)}`,
              `frame ${emaMs.toFixed(1)} ms`,
              `quality ${stats.level}${live.current.quality === "auto" ? " (auto)" : ""}`,
              `neurons ${stats.neurons}  edges ${stats.edges}  pulses ${stats.pulses}`,
            ].join("\n");
          }
        }
      },
    });
    loop.start();
    return () => loop.stop();
  }, [invalidate, statsRef, overlayRef]);

  return null;
}

interface BoundaryProps {
  fallback: ReactNode;
  children: ReactNode;
}

class WebglBoundary extends Component<BoundaryProps, { failed: boolean }> {
  override state = { failed: false };

  static getDerivedStateFromError(): { failed: boolean } {
    return { failed: true };
  }

  override render(): ReactNode {
    return this.state.failed ? this.props.fallback : this.props.children;
  }
}

export function NeuralCore({
  state,
  levelRef,
  quality,
  fpsLimit,
  reducedMotion,
  compact,
  debug,
  unavailableText,
}: NeuralCoreProps) {
  const [autoLevel, setAutoLevel] = useState<QualityLevel>("medium");
  const level = resolveQuality(quality, autoLevel, compact);
  const data = useMemo(() => getModelData(level), [level]);
  const statsRef = useRef<Stats>({ neurons: 0, edges: 0, pulses: 0, level });
  const overlayRef = useRef<HTMLDivElement | null>(null);
  const power = powerPreferenceFor(quality);
  const dpr = dprFor(level, typeof window === "undefined" ? 1 : window.devicePixelRatio);

  return (
    <div className="neural-core">
      <WebglBoundary fallback={<p className="muted core-fallback">{unavailableText}</p>}>
        <Canvas
          key={power}
          frameloop="demand"
          dpr={dpr}
          flat
          linear
          gl={{ antialias: false, alpha: false, powerPreference: power }}
          camera={{ fov: FOV, near: 0.1, far: 30, position: [0, 0, CAMERA_Z] }}
          onCreated={({ gl }) => gl.setClearColor(BACKGROUND, 1)}
          style={{ position: "absolute", inset: 0, pointerEvents: "none" }}
        >
          <Brain data={data} state={state} levelRef={levelRef} reducedMotion={reducedMotion} statsRef={statsRef} />
          <Driver
            state={state}
            fpsLimit={fpsLimit}
            quality={quality}
            compact={compact}
            autoLevel={autoLevel}
            onAutoLevel={setAutoLevel}
            statsRef={statsRef}
            overlayRef={overlayRef}
          />
        </Canvas>
      </WebglBoundary>
      {debug && <div className="core-debug mono" ref={overlayRef} />}
    </div>
  );
}
