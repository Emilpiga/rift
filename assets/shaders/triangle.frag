#version 450

layout(binding = 0) uniform UniformData {
    mat4 view;
    mat4 proj;
    vec4 cameraPos;
    vec4 lightDir;
    vec4 lightColor;
    vec4 fogColor;
    vec4 fogParams; // x = start, y = end
    vec4 pointLightPos[8];   // xyz = position, w = radius
    vec4 pointLightColor[8]; // xyz = color, w = intensity
    vec4 pointLightCount;    // x = count
    mat4 lightVP;            // directional light view-projection (for shadow map)
} ubo;

layout(set = 0, binding = 1) uniform sampler2D unusedSampler; // legacy slot, kept for descriptor compatibility
layout(set = 0, binding = 2) uniform sampler2DShadow shadowMap;
layout(set = 1, binding = 0) uniform sampler2D texSampler;

layout(location = 0) in vec3 fragWorldPos;
layout(location = 1) in vec3 fragNormal;
layout(location = 2) in vec3 fragColor;
layout(location = 3) in vec2 fragUV;

layout(location = 0) out vec4 outColor;

void main() {
    vec3 N = normalize(fragNormal);
    vec3 L = normalize(ubo.lightDir.xyz);
    vec3 V = normalize(ubo.cameraPos.xyz - fragWorldPos);
    vec3 H = normalize(L + V);

    // Ambient
    float ambient = ubo.lightColor.w;

    // Diffuse — quantized into 3 bands for cel-shading.
    float diffRaw = max(dot(N, L), 0.0);
    // Bands: shadow (0.30), mid (0.62), lit (0.85). Top band is no longer
    // 1.0 so directly-lit walls don't blow out and lose their texture.
    float diff;
    if (diffRaw < 0.30) {
        diff = mix(0.30, 0.62, smoothstep(0.25, 0.30, diffRaw));
    } else if (diffRaw < 0.65) {
        diff = mix(0.62, 0.85, smoothstep(0.60, 0.65, diffRaw));
    } else {
        diff = 0.85;
    }

    // Specular: thresholded toon highlight, very tight and dim. Killed
    // entirely on near-horizontal surfaces (floors / table tops) so dry
    // stone doesn't read as wet.
    float specRaw = pow(max(dot(N, H), 0.0), 128.0);
    float floorMask = smoothstep(0.80, 0.97, N.y);   // 0 walls, 1 floor up
    float spec = smoothstep(0.75, 0.82, specRaw) * (1.0 - floorMask);

    // Fresnel rim — very subtle, only on near-grazing vertical surfaces.
    // Killed on floors so distant tiles don't catch the moonlight.
    float fres = pow(1.0 - max(dot(N, V), 0.0), 5.0);
    vec3 rim = ubo.lightColor.rgb * fres * 0.18 * (1.0 - floorMask);

    vec3 texColor = texture(texSampler, fragUV).rgb;
    vec3 baseColor = fragColor * texColor;

    // ---- Directional shadow map (sampler2DShadow does the depth compare) ----
    // Project worldPos into light clip space, divide, remap [-1,1] -> [0,1].
    vec4 lightClip = ubo.lightVP * vec4(fragWorldPos, 1.0);
    vec3 lightNDC = lightClip.xyz / max(lightClip.w, 1e-5);
    vec3 shadowUV = vec3(lightNDC.xy * 0.5 + 0.5, lightNDC.z);
    // Bias scaled by surface slope to fight shadow acne.
    float NdotL = max(dot(N, L), 0.0);
    float bias = max(0.0008 * (1.0 - NdotL), 0.0002);
    shadowUV.z -= bias;

    // Skip sampling outside the shadow map (treat as fully lit).
    float shadow = 1.0;
    if (shadowUV.x >= 0.0 && shadowUV.x <= 1.0 &&
        shadowUV.y >= 0.0 && shadowUV.y <= 1.0 &&
        shadowUV.z >= 0.0 && shadowUV.z <= 1.0) {
        // 4-tap PCF for soft edges.
        vec2 texelSize = 1.0 / vec2(textureSize(shadowMap, 0));
        float s = 0.0;
        s += texture(shadowMap, vec3(shadowUV.xy + vec2(-0.5,-0.5)*texelSize, shadowUV.z));
        s += texture(shadowMap, vec3(shadowUV.xy + vec2( 0.5,-0.5)*texelSize, shadowUV.z));
        s += texture(shadowMap, vec3(shadowUV.xy + vec2(-0.5, 0.5)*texelSize, shadowUV.z));
        s += texture(shadowMap, vec3(shadowUV.xy + vec2( 0.5, 0.5)*texelSize, shadowUV.z));
        shadow = s * 0.25;
        // Lift the floor for shadowed surfaces, but keep them noticeably
        // darker than lit ones so the dungeon feels properly enclosed.
        shadow = mix(0.30, 1.0, shadow);
    }

    vec3 lighting = baseColor * ambient
                  + baseColor * diff * ubo.lightColor.rgb * shadow
                  + ubo.lightColor.rgb * spec * 0.10 * shadow
                  + rim;

    // Point lights
    int numLights = int(ubo.pointLightCount.x);
    for (int i = 0; i < numLights && i < 8; i++) {
        vec3 lightPos = ubo.pointLightPos[i].xyz;
        float radius = ubo.pointLightPos[i].w;
        vec3 lightCol = ubo.pointLightColor[i].xyz;
        float intensity = ubo.pointLightColor[i].w;

        vec3 toLight = lightPos - fragWorldPos;
        float dist = length(toLight);
        if (dist < radius) {
            float atten = 1.0 - (dist / radius);
            atten = atten * atten; // quadratic falloff
            vec3 Lp = normalize(toLight);
            float diffPRaw = max(dot(N, Lp), 0.0);
            // Quantize point-light diffuse to 2 bands (matches main toon look).
            float diffP = (diffPRaw < 0.5)
                ? mix(0.0, 0.6, smoothstep(0.45, 0.5, diffPRaw))
                : 1.0;
            vec3 Hp = normalize(Lp + V);
            float specPRaw = pow(max(dot(N, Hp), 0.0), 64.0);
            float specP = smoothstep(0.70, 0.78, specPRaw) * (1.0 - floorMask);
            lighting += baseColor * diffP * lightCol * intensity * atten;
            lighting += lightCol * specP * intensity * atten * 0.12;
        }
    }

    // Distance fog
    float dist = length(ubo.cameraPos.xyz - fragWorldPos);
    float fogFactor = clamp((dist - ubo.fogParams.x) / (ubo.fogParams.y - ubo.fogParams.x), 0.0, 1.0);
    // Smooth curve for more natural falloff
    fogFactor = fogFactor * fogFactor;
    vec3 finalColor = mix(lighting, ubo.fogColor.rgb, fogFactor);

    outColor = vec4(finalColor, 1.0);
}
