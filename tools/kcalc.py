"""K и очки по формуле scoring.py (заморожена 28.10): python3 kcalc.py <track> <T_s> <M_MB> <S_lines> [deps=0]"""
import math, sys
REF = {"a": (32.047276, 227983360, 56), "b": (56.352376, 2199961600, 91),
       "c": (14.444059, 367435776, 108), "d": (72.692655, 892915712, 43)}  # calibration-seed1.json, reference_axes.json
t, T, M, S = sys.argv[1], float(sys.argv[2]), float(sys.argv[3]) * 1e6, float(sys.argv[4])
D = 1 + (int(sys.argv[5]) if len(sys.argv) > 5 else 0)
tr, mr, sr = REF[t]
f = {"T": (tr / T) ** 0.5, "M": min(mr / M, 50) ** 0.2, "S": min(sr / S, 1) ** 0.15, "D": (1 / D) ** 0.15}
K = math.prod(f.values())
pts = max(0.0, min(100.0, 50 * math.log10(K)))
print(" ".join(f"{a}={v:.3f}" for a, v in f.items()), f"K={K:.2f} speed_points={pts:.1f} machine_part={0.65*pts:.1f}")
