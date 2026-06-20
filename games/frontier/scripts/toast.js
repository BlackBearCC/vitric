// 通知/toast:动作事件来时显一行字(toast.json 规则设 content+timer),本系统每帧倒计时,到点清空。
vitric.system("toast-tick", { query: ["Toast", "UiLabel"], writes: ["Toast", "UiLabel"] }, (entities, ctx) => {
  for (const e of entities) {
    if ((e.Toast.timer || 0) > 0) {
      e.Toast.timer = e.Toast.timer - ctx.dt;
      if (e.Toast.timer <= 0) { e.Toast.timer = 0; if (e.UiLabel.content !== "") e.UiLabel.content = ""; }
    }
  }
});
