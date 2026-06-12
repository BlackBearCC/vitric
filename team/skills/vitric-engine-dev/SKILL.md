---
name: vitric-engine-dev
description: 引擎开发纪律:修改 Vitric 引擎本体(crates/)时的强制约束——确定性铁律、双路径镜像、测试锁死。改引擎代码前必读。
---

# 引擎开发工单(改 crates/ 必守)

## 确定性铁律(违反即否决)
- 模拟状态演化只允许:输入流/回复流(都录进录像)、组件状态、PCG32。禁止墙钟/HashMap迭代/线程时序影响状态。
- 一切跨 tick 状态进组件或进快照(GameLogic snapshot_state 钩子);漏一项=重放静默分歧。
- 渲染装饰(抖屏/光照/阴影)只许读状态,偏移必须是 (tick,状态) 的纯函数,不碰模拟 RNG 流。

## 双路径镜像
- CPU(vitric-render)是真相源:截图可断言、逐字节确定。GPU(gpu.rs)视觉对齐,公式逐句同构,naga 离线验 WGSL。
- 浮点跨 JS 边界走 $f64 位串(QuickJS dtoa 不可信);跨平台只保证同平台同二进制。

## 工程纪律
- 错误显式带修法提示,一次报全;不写 fallback,commit-on-success;空门禁/静默放行=后门,一律硬错误。
- 每个特性:回归测试锁死(新测试必须先证明在旧代码上失败)、向后兼容字节锁(开关关闭=原行为)、
  clippy -D warnings 零告警、x86_64-pc-windows-gnu 交叉编译通过、agent-guide 中英同步。
- 约定组件模式:引擎认名字,字段用户 schema 定义,缺字段显式报错(参考 Body/Light/Bloom)。
