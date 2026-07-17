// Toast/notify: when an action event arrives, show one line of text (toast.json rule sets content+timer); this system counts down every frame and clears when it reaches zero.
vitric.system("toast-tick", { query: ["Toast", "UiLabel"], writes: ["Toast", "UiLabel"] }, (entities, ctx) => {
  for (const e of entities) {
    if ((e.Toast.timer || 0) > 0) {
      e.Toast.timer = e.Toast.timer - ctx.dt;
      if (e.Toast.timer <= 0) { e.Toast.timer = 0; if (e.UiLabel.content !== "") e.UiLabel.content = ""; }
    }
  }
});
