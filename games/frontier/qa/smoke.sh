#!/usr/bin/env bash
# frontier 核心机制冒烟测试(无头,报 PASS/FAIL,全过 exit 0)。
# 用法: bash games/frontier/qa/smoke.sh
# 需要: 已 cargo build --release -p vitric-cli;python3(curl 已假定有)。
set -u
cd "$(dirname "$0")/../../.." || exit 2
BIN=./target/release/vitric
PORT=6390
LLMP=6391
PASS=0; FAIL=0
ok(){ echo "  PASS $1"; PASS=$((PASS+1)); }
no(){ echo "  FAIL $1"; FAIL=$((FAIL+1)); }

# 清理本测试在这两个端口上可能残留的进程(启动前也清一遍,免得端口被占)
port_clean(){ ps -eo pid,comm,args 2>/dev/null | awk -v a="--port $PORT" -v b="fake_llm.py $LLMP" '($2=="vitric" && index($0,a)) || ($0 ~ /python/ && index($0,b)) {print $1}' | xargs -r kill -9 2>/dev/null; }
cleanup(){ port_clean; kill "${FLM:-0}" "${SRV:-0}" 2>/dev/null; }
trap cleanup EXIT
port_clean; sleep 1

echo "== check =="
if $BIN check games/frontier >/dev/null 2>&1; then ok "vitric check 绿"; else no "vitric check"; echo "  (check 不过,后面跳过)"; echo "结果: PASS=$PASS FAIL=$FAIL"; exit 1; fi

# 字体没生成只警告(中文不显但不挡逻辑)
[ -f games/frontier/fonts/cjk.otf ] || echo "  WARN 没有 fonts/cjk.otf(跑 tools/gen_font.py 生成,否则中文不显)"

python3 games/frontier/tools/fake_llm.py "$LLMP" >/dev/null 2>&1 & FLM=$!
sleep 1
VITRIC_LLM_URL="http://127.0.0.1:$LLMP/v1/chat/completions" VITRIC_LLM_KEY=x VITRIC_LLM_MODEL=stub \
  $BIN run games/frontier --port "$PORT" >/tmp/frontier_smoke.log 2>&1 & SRV=$!
sleep 2
if grep -qE "Address already|绑定失败|错误:" /tmp/frontier_smoke.log; then no "run 启动(端口占用?见 /tmp/frontier_smoke.log)"; echo "结果: PASS=$PASS FAIL=$FAIL"; exit 1; fi

rpc(){ curl -s -X POST "http://127.0.0.1:$PORT/rpc" -d "$1"; }
field(){ rpc "{\"method\":\"world/get\",\"params\":{\"entity\":\"$1\"}}" | python3 -c "import sys,json;d=json.load(sys.stdin);print(d['result']['components']$2 if d.get('ok') else 'GONE')" 2>/dev/null; }

# 全程实时跑(不 pause/step),好让异步 LLM 调用和到来计时自然进行。
echo "== 建设 + 生存:建 quarters/plot,实时看氧回升 =="
rpc '{"method":"input/inject","params":{"action":"5"}}' >/dev/null; sleep 0.2  # quarters:留住原伙伴
rpc '{"method":"input/click","params":{"x":6,"y":6,"button":"left"}}' >/dev/null; sleep 0.2
rpc '{"method":"input/inject","params":{"action":"3"}}' >/dev/null; sleep 0.2   # plot:产氧
rpc '{"method":"input/click","params":{"x":10,"y":7,"button":"left"}}' >/dev/null; sleep 0.5
o2a=$(field @colony "['Colony']['oxygen']")
c0=$(field @colony "['Census']['count']")
sleep 12  # 实时:plot 让氧回升;到来计时(8s)到点招一个新伙伴
o2b=$(field @colony "['Colony']['oxygen']")
c1=$(field @colony "['Census']['count']")
python3 -c "import sys; sys.exit(0 if float('$o2b')>=float('$o2a') else 1)" 2>/dev/null && ok "生存:建 plot 后氧不降反升 ($o2a -> $o2b)" || no "生存:氧没回升 ($o2a -> $o2b)"

echo "== 伙伴到来:计数随时间增长 =="
python3 -c "import sys; sys.exit(0 if int('$c1' or 0)>int('$c0' or 0) else 1)" 2>/dev/null && ok "到来:人数 $c0 -> $c1" || no "到来:人数没涨 ($c0 -> $c1)"

echo "== 活伙伴:搭话走 ctx.ask =="
rpc '{"method":"input/inject","params":{"action":"t"}}' >/dev/null; sleep 1.5
said=$(field @companion "['Text']['content']")
[ -n "$said" ] && [ "$said" != "GONE" ] && ok "伙伴搭话有回复 (\"${said:0:14}…\")" || no "伙伴没回话(LLM 链路?)"

echo "== HUD:屏上资源条非空 =="
hud=$(field @hud_res "['Text']['content']")
[ -n "$hud" ] && [ "$hud" != "GONE" ] && ok "HUD 有内容 (\"$hud\")" || no "HUD 空"

echo
echo "结果: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
