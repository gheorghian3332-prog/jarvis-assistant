/** GLSL for the four draw calls. Everything animated is a function of the uniforms below. */

const COMMON = /* glsl */ `
uniform float u_time;
uniform float u_pulseClock;
uniform float u_wavePhase;
uniform float u_viewScale;
uniform float u_sizeScale;
uniform float u_camDist;
uniform float u_expand;
uniform float u_level;
uniform float u_jitter;
uniform float u_waveAmt;
uniform float u_waveDir;
uniform float u_flicker;
uniform float u_throb;
uniform float u_flash;
uniform float u_intensity;
uniform vec3 u_color;

// Wave position of a point in 0..1: radial distance from the centre or height from the bottom.
float waveGlow(vec3 p) {
  float radial = clamp(length(p) / 1.1, 0.0, 1.0);
  float height = clamp((p.y + 1.05) / 1.81, 0.0, 1.0);
  float coord = mix(radial, height, u_waveDir);
  float d = abs(fract(coord - u_wavePhase + 0.5) - 0.5);
  return exp(-pow(d / 0.11, 2.0)) * u_waveAmt;
}

// 1 for the near side of the structure, 0.25 for the far side.
float depthFade(float viewZ) {
  return mix(0.25, 1.0, smoothstep(u_camDist + 1.2, u_camDist - 1.2, -viewZ));
}
`;

export const NEURON_VERTEX = /* glsl */ `
${COMMON}
attribute float a_size;
attribute float a_phase;
attribute float a_bright;
varying float v_glow;

void main() {
  vec3 p = position * (1.0 + u_expand);
  p += u_jitter * vec3(
    sin(u_time * 37.0 + a_phase * 11.0),
    sin(u_time * 41.0 + a_phase * 7.0),
    sin(u_time * 29.0 + a_phase * 5.0)
  );
  vec4 mv = modelViewMatrix * vec4(p, 1.0);
  float fade = depthFade(mv.z);
  float twinkle = 0.75 + 0.25 * sin(u_time * (0.8 + a_bright) + a_phase);
  float wave = waveGlow(position);
  float off = step(0.5, fract(sin(floor(u_time * 14.0) + a_phase * 91.0) * 43758.5453));
  float flick = 1.0 - u_flicker * off;
  float voice = u_level;
  v_glow = a_bright * twinkle * (1.0 + wave * 1.6 + voice * 1.8) * flick * u_throb * fade;
  float size = a_size * (1.0 + wave * 0.8 + voice * 1.0) * mix(0.6, 1.0, fade);
  gl_PointSize = clamp(size * u_sizeScale * u_viewScale / -mv.z, 1.0, 96.0);
  gl_Position = projectionMatrix * mv;
}
`;

export const NEURON_FRAGMENT = /* glsl */ `
uniform vec3 u_color;
uniform float u_intensity;
uniform float u_flash;
varying float v_glow;

void main() {
  float d = length(gl_PointCoord - 0.5) * 2.0;
  if (d > 1.0) discard;
  float core = exp(-d * d * 4.5);
  float halo = pow(1.0 - d, 1.6) * 0.65;
  float alpha = (core + halo) * v_glow * u_intensity * (1.0 + 2.0 * u_flash);
  vec3 color = mix(u_color, vec3(1.0), core * 0.5 + u_flash * 0.5);
  gl_FragColor = vec4(color, alpha);
}
`;

export const LINE_VERTEX = /* glsl */ `
${COMMON}
attribute float a_hub;
varying float v_alpha;

void main() {
  vec3 p = position * (1.0 + u_expand);
  vec4 mv = modelViewMatrix * vec4(p, 1.0);
  float wave = waveGlow(position);
  float base = 0.2 + a_hub * 0.18 + wave * 0.45 + u_level * 0.25;
  v_alpha = base * u_throb * depthFade(mv.z);
  gl_Position = projectionMatrix * mv;
}
`;

export const LINE_FRAGMENT = /* glsl */ `
uniform vec3 u_color;
uniform float u_intensity;
uniform float u_flash;
varying float v_alpha;

void main() {
  vec3 color = mix(u_color, vec3(1.0), 0.12 + u_flash * 0.5);
  gl_FragColor = vec4(color, v_alpha * u_intensity * (1.0 + 2.0 * u_flash));
}
`;

export const PULSE_VERTEX = /* glsl */ `
${COMMON}
uniform float u_density;
attribute vec3 a_start;
attribute vec3 a_end;
attribute float a_offset;
attribute float a_speed;
attribute float a_tail;
varying float v_alpha;

void main() {
  float hash = fract(a_offset * 17.0 + 0.37);
  float visible = u_density < 0.001 ? 0.0 : 1.0 - smoothstep(u_density - 0.04, u_density, hash);
  float head = fract(u_pulseClock * a_speed + a_offset);
  float t = head - a_tail * 0.05;
  visible *= step(0.0, t);
  vec3 at = mix(a_start, a_end, clamp(t, 0.0, 1.0));
  vec3 p = at * (1.0 + u_expand);
  vec4 mv = modelViewMatrix * vec4(p, 1.0);
  float wave = waveGlow(at);
  float taper = 1.0 - a_tail * 0.3;
  float fade = depthFade(mv.z);
  v_alpha = visible * sin(3.14159265 * clamp(t, 0.0, 1.0)) * (1.0 + wave) * taper * taper * u_throb * fade;
  float size = (1.0 + wave * 0.6) * taper * mix(0.7, 1.0, fade);
  gl_PointSize = visible > 0.0 ? clamp(size * u_viewScale * 0.06 / -mv.z, 2.0, 40.0) : 0.0;
  gl_Position = visible > 0.0 ? projectionMatrix * mv : vec4(2.0, 2.0, 2.0, 1.0);
}
`;

export const PULSE_FRAGMENT = /* glsl */ `
uniform vec3 u_color;
uniform float u_intensity;
uniform float u_flash;
varying float v_alpha;

void main() {
  float d = length(gl_PointCoord - 0.5) * 2.0;
  if (d > 1.0) discard;
  float core = exp(-d * d * 4.0);
  vec3 color = mix(u_color, vec3(1.0), 0.6 + u_flash * 0.3);
  gl_FragColor = vec4(color, min(1.0, core * v_alpha * u_intensity * 1.8));
}
`;

/** One camera-facing quad at the centre with a radial gradient: the soft inner glow. */
export const GLOW_VERTEX = /* glsl */ `
${COMMON}
varying vec2 v_uv;

void main() {
  vec4 mv = modelViewMatrix * vec4(0.0, -0.1, 0.0, 1.0);
  mv.xy += position.xy * 1.6 * (1.0 + u_expand);
  v_uv = position.xy;
  gl_Position = projectionMatrix * mv;
}
`;

export const GLOW_FRAGMENT = /* glsl */ `
uniform vec3 u_color;
uniform float u_intensity;
uniform float u_flash;
uniform float u_level;
uniform float u_throb;
varying vec2 v_uv;

void main() {
  float d = length(v_uv);
  float falloff = exp(-d * d * 3.5) * (1.0 - smoothstep(0.8, 1.0, d));
  float alpha = falloff * 0.2 * u_intensity * (0.7 + u_level * 1.2) * u_throb * (1.0 + 2.0 * u_flash);
  gl_FragColor = vec4(mix(u_color, vec3(1.0), 0.15), alpha);
}
`;
