# echo sound effect synth (deterministic: pure stdlib, random fixed seed). Re-run rebuilds all wav.
# Recipe convention: 16-bit PCM / 44100Hz / mono; envelope fade in/out >=5ms at both ends to prevent pops; peak <=0.7 to prevent clipping.
import math, random, struct, wave, os

SR = 44100
HERE = os.path.dirname(os.path.abspath(__file__))

def write_wav(name, samples, peak=0.7):
    m = max(max(samples), -min(samples), 1e-9)
    k = peak / m
    fade = int(SR * 0.006)
    n = len(samples)
    out = []
    for i, s in enumerate(samples):
        g = 1.0
        if i < fade: g = i / fade
        if i >= n - fade: g = min(g, (n - 1 - i) / fade)
        out.append(int(max(-1, min(1, s * k * g)) * 32767))
    with wave.open(os.path.join(HERE, name), "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(SR)
        w.writeframes(struct.pack("<%dh" % n, *out))
    print(name, round(n / SR, 2), "s")

def env_exp(i, n, k=5.0):
    return math.exp(-k * i / n)

rng = random.Random(42)
noise_buf = [rng.uniform(-1, 1) for _ in range(SR * 2)]

# click —— UI select: 880Hz short sine blip
n = int(SR * 0.05)
write_wav("click.wav", [math.sin(2 * math.pi * 880 * i / SR) * env_exp(i, n, 6) for i in range(n)])

# card —— card play sweep: white noise high-pass feel + frequency down-sweep
n = int(SR * 0.12)
buf = []
for i in range(n):
    t = i / n
    f = 1400 - 900 * t
    s = 0.5 * math.sin(2 * math.pi * f * i / SR) + 0.5 * (noise_buf[i] - noise_buf[i - 1]) * 3
    buf.append(s * env_exp(i, n, 4))
write_wav("card.wav", buf)

# lamp —— place lamp: warm two-note arpeggio 440→660
n = int(SR * 0.14)
buf = []
for i in range(n):
    f = 440 if i < n // 2 else 660
    j = i if i < n // 2 else i - n // 2
    buf.append(math.sin(2 * math.pi * f * i / SR) * env_exp(j, n // 2, 4))
write_wav("lamp.wav", buf)

# hiss —— shadow creature action hiss: low-pass noise
n = int(SR * 0.22)
buf, lp = [], 0.0
for i in range(n):
    lp += 0.12 * (noise_buf[i] - lp)
    buf.append(lp * (1 - abs(2 * i / n - 1)))
write_wav("hiss.wav", buf)

# stun —— light burn stiffen: high-frequency sizzle + noise
n = int(SR * 0.18)
buf = []
for i in range(n):
    s = 0.6 * math.sin(2 * math.pi * 1760 * i / SR) * (1 if (i // 300) % 2 else 0.3) + 0.4 * noise_buf[i]
    buf.append(s * env_exp(i, n, 3))
write_wav("stun.wav", buf)

# hit —— player hit: sawtooth descending 400→120
n = int(SR * 0.15)
buf = []
ph = 0.0
for i in range(n):
    f = 400 - 280 * i / n
    ph += f / SR
    buf.append((2 * (ph % 1) - 1) * env_exp(i, n, 3))
write_wav("hit.wav", buf)

# die —— shadow creature dissipate: 200→60 descent + noise tail
n = int(SR * 0.22)
buf = []
ph = 0.0
for i in range(n):
    f = 200 - 140 * i / n
    ph += f / SR
    buf.append((math.sin(2 * math.pi * ph) * 0.8 + noise_buf[i] * 0.2) * env_exp(i, n, 3.5))
write_wav("die.wav", buf)

# devour —— Boss devour lamp: 150→55 low growl
n = int(SR * 0.2)
buf = []
ph = 0.0
for i in range(n):
    f = 150 - 95 * i / n
    ph += f / SR
    buf.append(math.sin(2 * math.pi * ph) * (1 - 0.3 * math.sin(2 * math.pi * 18 * i / SR)) * env_exp(i, n, 2.5))
write_wav("devour.wav", buf)

# reject —— invalid card play: short double low-tone buzz
n = int(SR * 0.1)
write_wav("reject.wav", [math.sin(2 * math.pi * 160 * i / SR) * (1 if i < n // 2 else 0.6) * env_exp(i % (n // 2), n // 2, 4) for i in range(n)])

# win —— victory: major chord arpeggio ascending 523/659/784/1046
notes = [523.25, 659.25, 783.99, 1046.5]
seg = int(SR * 0.12)
buf = []
for k, f in enumerate(notes):
    for i in range(seg):
        buf.append(math.sin(2 * math.pi * f * i / SR) * env_exp(i, seg, 2.5))
tail = int(SR * 0.3)
for i in range(tail):
    s = sum(math.sin(2 * math.pi * f * i / SR) for f in notes) / 4
    buf.append(s * env_exp(i, tail, 3))
write_wav("win.wav", buf)

# lose —— defeat: minor descending three-note
notes = [392.0, 311.13, 261.63]
seg = int(SR * 0.22)
buf = []
for f in notes:
    for i in range(seg):
        buf.append(math.sin(2 * math.pi * f * i / SR) * env_exp(i, seg, 2.0))
write_wav("lose.wav", buf)

# ---- BGM: align head/tail samples to avoid loop clicks (each chord segment carries its own attack/release envelope, segment length divides evenly) ----

def chord(fs, dur, vol=1.0, pulse=0):
    n = int(SR * dur)
    out = []
    atk = int(SR * 0.05)
    for i in range(n):
        g = min(1, i / atk, (n - 1 - i) / atk)
        s = sum(math.sin(2 * math.pi * f * i / SR) for f in fs) / len(fs)
        if pulse:
            s *= 0.6 + 0.4 * math.sin(2 * math.pi * pulse * i / SR)
        out.append(s * g * vol)
    return out

# bgm-menu —— serene: Am F C G bass arpeggio pad, 2.4s×4 = 9.6s loop
prog = [[220.0, 261.63, 329.63], [174.61, 220.0, 261.63], [130.81, 196.0, 261.63], [196.0, 246.94, 293.66]]
buf = []
for fs in prog:
    buf += chord(fs, 2.4, vol=0.8)
write_wav("bgm-menu.wav", buf, peak=0.45)

# bgm-battle —— tense: D minor pulse ostinato + tritone pad, 1.8s×4 = 7.2s loop
prog = [[146.83, 220.0], [146.83, 207.65], [138.59, 220.0], [146.83, 233.08]]
buf = []
for fs in prog:
    seg = chord(fs, 1.8, vol=0.7, pulse=4)
    # layer an eighth-note bass pulse
    step = int(SR * 0.225)
    for k in range(8):
        f0 = fs[0] / 2
        for i in range(int(step * 0.6)):
            idx = k * step + i
            if idx < len(seg):
                seg[idx] += 0.5 * math.sin(2 * math.pi * f0 * i / SR) * env_exp(i, int(step * 0.6), 4)
    buf += seg
write_wav("bgm-battle.wav", buf, peak=0.45)
