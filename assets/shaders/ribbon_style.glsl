// Ribbon art-direction — `style_pack` / `style_aux` match particle `unpackGfxStyle`.
// Preset ids: 1 = void frost, 2 = ember, 3 = arc (see `style::gpu_id` in Rust).

void applyRibbonStyle(
    vec4 stylePack,
    vec4 styleAux,
    float u,
    float v,
    float vTime,
    inout vec3 rgb,
    inout float a
) {
    float presetId = stylePack.x;
    if (presetId < 0.5) {
        return;
    }
    float turbulence = (styleAux.x > 0.0) ? styleAux.x : 1.0;
    float edgeSoft = (styleAux.z > 0.0) ? styleAux.z : 0.25;
    if (presetId < 1.5) {
        float col = 1.0 - smoothstep(0.10, 0.90, abs(u - 0.5) * 2.0);
        rgb *= mix(vec3(0.90, 0.97, 1.10), vec3(1.0), col);
        float tight = exp(-pow((u - 0.5) * mix(2.4, 3.6, 1.0 - edgeSoft), 2.0));
        float wide = exp(-pow((u - 0.5) * 2.6, 2.0));
        a *= mix(1.0, tight / max(wide, 1e-4), 0.45);
        rgb *= 0.92 + 0.08 * sin(v * 38.0 - vTime * 6.0);
    } else if (presetId < 2.5) {
        rgb *= mix(vec3(1.12, 0.94, 0.82), vec3(1.0), smoothstep(0.25, 0.75, v));
        float heat = valueNoise(vec2(u * 24.0, v * 8.0 + vTime * 0.8));
        rgb *= mix(0.94, 1.08, heat);
        a *= mix(1.0, 1.06, turbulence * 0.08);
    } else if (presetId < 3.5) {
        float fil = valueNoise(vec2(u * 55.0 + vTime * 3.5, v * 14.0));
        rgb *= mix(0.90, 1.14, fil);
        float core = exp(-pow((u - 0.5) * mix(2.8, 3.8, edgeSoft), 2.0));
        rgb += core * vec3(0.15, 0.08, 0.22) * fil;
    }
}
