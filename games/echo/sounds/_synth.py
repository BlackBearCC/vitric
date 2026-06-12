# echo 音效合成（确定性：纯 stdlib，random 固定种子）。重跑即重建全部 wav。
# 配方约定：16-bit PCM / 44100Hz / 单声道；包络首尾淡入淡出 >=5ms 防爆音；峰值 <=0.7 防削波。
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

# click —— UI 选中：880Hz 短正弦泡
n = int(SR * 0.05)
write_wav("click.wav", [math.sin(2 * math.pi * 880 * i / SR) * env_exp(i, n, 6) for i in range(n)])

# card —— 出牌挥扫：白噪声高通感 + 频率下扫
n = int(SR * 0.12)
buf = []
for i in range(n):
    t = i / n
    f = 1400 - 900 * t
    s = 0.5 * math.sin(2 * math.pi * f * i / SR) + 0.5 * (noise_buf[i] - noise_buf[i - 1]) * 3
    buf.append(s * env_exp(i, n, 4))
write_wav("card.wav", buf)

# lamp —— 放灯：暖两音琶音 440→660
n = int(SR * 0.14)
buf = []
for i in range(n):
    f = 440 if i < n // 2 else 660
    j = i if i < n // 2 else i - n // 2
    buf.append(math.sin(2 * math.pi * f * i / SR) * env_exp(j, n // 2, 4))
write_wav("lamp.wav", buf)

# hiss —— 影怪行动嘶声：低通噪声
n = int(SR * 0.22)
buf, lp = [], 0.0
for i in range(n):
    lp += 0.12 * (noise_buf[i] - lp)
    buf.append(lp * (1 - abs(2 * i / n - 1)))
write_wav("hiss.wav", buf)

# stun —— 光灼僵直：高频滋滋 + 噪声
n = int(SR * 0.18)
buf = []
for i in range(n):
    s = 0.6 * math.sin(2 * math.pi * 1760 * i / SR) * (1 if (i // 300) % 2 else 0.3) + 0.4 * noise_buf[i]
    buf.append(s * env_exp(i, n, 3))
write_wav("stun.wav", buf)

# hit —— 玩家受击：锯齿下行 400→120
n = int(SR * 0.15)
buf = []
ph = 0.0
for i in range(n):
    f = 400 - 280 * i / n
    ph += f / SR
    buf.append((2 * (ph % 1) - 1) * env_exp(i, n, 3))
write_wav("hit.wav", buf)

# die —— 影怪消散：200→60 沉落 + 噪声尾
n = int(SR * 0.22)
buf = []
ph = 0.0
for i in range(n):
    f = 200 - 140 * i / n
    ph += f / SR
    buf.append((math.sin(2 * math.pi * ph) * 0.8 + noise_buf[i] * 0.2) * env_exp(i, n, 3.5))
write_wav("die.wav", buf)

# devour —— Boss 吞灯：150→55 低吼
n = int(SR * 0.2)
buf = []
ph = 0.0
for i in range(n):
    f = 150 - 95 * i / n
    ph += f / SR
    buf.append(math.sin(2 * math.pi * ph) * (1 - 0.3 * math.sin(2 * math.pi * 18 * i / SR)) * env_exp(i, n, 2.5))
write_wav("devour.wav", buf)

# reject —— 出牌无效：短促双低音蜂鸣
n = int(SR * 0.1)
write_wav("reject.wav", [math.sin(2 * math.pi * 160 * i / SR) * (1 if i < n // 2 else 0.6) * env_exp(i % (n // 2), n // 2, 4) for i in range(n)])

# win —— 胜利：大三和弦琶音上行 523/659/784/1046
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

# lose —— 失败：小调下行三音
notes = [392.0, 311.13, 261.63]
seg = int(SR * 0.22)
buf = []
for f in notes:
    for i in range(seg):
        buf.append(math.sin(2 * math.pi * f * i / SR) * env_exp(i, seg, 2.0))
write_wav("lose.wav", buf)

# ---- BGM：首尾样本对齐避免循环咔哒（每段和弦自带起落包络，段长整除） ----

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

# bgm-menu —— 静谧：Am F C G 低音琶音垫，2.4s×4 = 9.6s 循环
prog = [[220.0, 261.63, 329.63], [174.61, 220.0, 261.63], [130.81, 196.0, 261.63], [196.0, 246.94, 293.66]]
buf = []
for fs in prog:
    buf += chord(fs, 2.4, vol=0.8)
write_wav("bgm-menu.wav", buf, peak=0.45)

# bgm-battle —— 紧张：D 小调脉冲 ostinato + 三全音垫，1.8s×4 = 7.2s 循环
prog = [[146.83, 220.0], [146.83, 207.65], [138.59, 220.0], [146.83, 233.08]]
buf = []
for fs in prog:
    seg = chord(fs, 1.8, vol=0.7, pulse=4)
    # 叠一条八分音符低音脉冲
    step = int(SR * 0.225)
    for k in range(8):
        f0 = fs[0] / 2
        for i in range(int(step * 0.6)):
            idx = k * step + i
            if idx < len(seg):
                seg[idx] += 0.5 * math.sin(2 * math.pi * f0 * i / SR) * env_exp(i, int(step * 0.6), 4)
    buf += seg
write_wav("bgm-battle.wav", buf, peak=0.45)
