# Pelago

用 Rust 写的简易网页浏览器。打开一个 URL，或直接渲染一段 HTML。

## 结构

```
src/
  main.rs          # 入口与命令行
  app/             # 窗口、地址栏等 UI
  engine/          # DOM / HTML / CSS / 布局 / 绘制 / 图片 / JS
  net/             # HTTP 拉取与 URL 工具
```

## 用法

```
cargo run
cargo run -- <url>
cargo run -- --html "<html>"
```

不带参数时会打开欢迎页，可在地址栏输入网址继续浏览。

## JavaScript

内嵌 [Boa](https://boajs.dev/) 引擎。会执行内联脚本和 `<script src>`，并提供 `location`、`navigator`、`console`、`setTimeout` / `setInterval`（回调立即执行一次），以及最小 DOM API：`getElementById`、`querySelector`、`querySelectorAll`、`getElementsByTagName`、`createElement`、`appendChild`、`textContent`、`className`、`innerHTML`、`addEventListener`（空实现）。

样式会下载 `<link rel="stylesheet">` 和 `@import`。选择器支持标签、class、id、后代和子代。布局支持 `flex`、`grid`、`float` 和 `position: absolute`。

`$`、`define`、`bds` 这类页面自己的全局变量，要等对应脚本成功执行后才会出现。当前没有真正的事件循环和完整 DOM。

## 构建

需要 [Rust](https://www.rust-lang.org/)（edition 2024）。

```
cargo build --release
```
