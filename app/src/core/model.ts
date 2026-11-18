import {
  buildEdges,
  edgeLineHubs,
  edgeLinePositions,
  expandPulseTails,
  generateNeurons,
  generatePulses,
  linkRadius,
  pulseCountFor,
  type NeuronData,
  type PulseData,
  type PulseTailData,
} from "./geometry";
import { QUALITY_SPECS, type QualityLevel } from "./quality";

/** Fixed seed: every start and every quality level of the same size produce the same brain. */
export const BRAIN_SEED = 0x4a525653;
export const NEIGHBOURS = 3;
/** Points per pulse: the head plus two trailing points. */
export const PULSE_TAILS = 3;

export interface ModelData {
  level: QualityLevel;
  neurons: NeuronData;
  edgeCount: number;
  linePositions: Float32Array;
  lineHubs: Float32Array;
  pulses: PulseData;
  /** The pulses repeated `PULSE_TAILS` times; this is what is drawn. */
  pulseTails: PulseTailData;
}

const cache = new Map<QualityLevel, ModelData>();

/** Generates the geometry data once per quality level and keeps it. */
export function getModelData(level: QualityLevel): ModelData {
  const cached = cache.get(level);
  if (cached) return cached;
  const spec = QUALITY_SPECS[level];
  const neurons = generateNeurons(spec.neurons, BRAIN_SEED);
  const edges = buildEdges(neurons.positions, NEIGHBOURS, linkRadius(spec.neurons));
  const pulses = generatePulses(
    neurons.positions,
    edges,
    pulseCountFor(edges.length / 2, spec.pulseCap),
    BRAIN_SEED + 1,
  );
  const data: ModelData = {
    level,
    neurons,
    edgeCount: edges.length / 2,
    linePositions: edgeLinePositions(neurons.positions, edges),
    lineHubs: edgeLineHubs(neurons.hubs, edges),
    pulses,
    pulseTails: expandPulseTails(pulses, PULSE_TAILS),
  };
  cache.set(level, data);
  return data;
}
