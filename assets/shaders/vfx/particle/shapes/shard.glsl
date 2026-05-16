// Shard: diamond SDF with derivative-smoothed edges, a bright
// rim, and a subtle internal facet line. Reads as a crystal
// chip rather than a flat polygon.
float shard(vec2 uv, float seed) {
    float ang = seed * 6.2831853;
    vec2 c = uv - 0.5;
    float ca = cos(ang), sa = sin(ang);
    vec2 r = vec2(ca * c.x - sa * c.y, sa * c.x + ca * c.y);
    float d = abs(r.x) + abs(r.y) * 1.6;     // diamond, slightly tall
    float body = 1.0 - aaStep(0.38, d);
    float rim = aaBand(d, 0.34, 0.035) * 0.72;
    float facet = aaBand(abs(r.x - r.y * 0.42), 0.0, 0.018)
                * (1.0 - aaStep(0.31, d)) * 0.25;
    float secondaryFacet = aaBand(abs(r.x + r.y * 0.58), 0.0, 0.014)
                         * (1.0 - aaStep(0.28, d)) * 0.16;
    return body + rim + facet + secondaryFacet;
}
