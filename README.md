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

内嵌 [Boa](https://boajs.dev/) 引擎，会执行页面中的**内联** `<script>`，并提供最小 `location` API（`href` / `assign` / `replace`）。

当前不支持完整 DOM、外部 `<script src>`、事件与定时器。

## 构建

需要 [Rust](https://www.rust-lang.org/)（edition 2024）。

```
cargo build --release
```
