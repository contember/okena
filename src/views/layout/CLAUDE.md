# layout/ — Terminal & App Layout System

## App Pane Layout Guidelines

**Rule 1**: Never use `size_full()` on a div that is a flex-column child. Use `flex_1().w_full().min_h_0()` instead. `size_full()` sets `height: 100%` (percentage-based) which doesn't resolve correctly in Taffy when the parent's height comes from the flex algorithm.

**Rule 2**: Every app pane must follow this layout scaffold:
```
root: flex_1(), w_full(), min_h_0(), flex(), flex_col()
├── header: flex_shrink_0()           [optional, fixed at top]
├── scroll body: id(), flex_1(), min_h_0(), overflow_y_scroll()
│   └── content div                   [unsized, grows with content]
└── footer: flex_shrink_0()           [optional, fixed at bottom]
```

**Rule 3**: `min_h_0()` must appear on EVERY flex-1 ancestor from the pane root down to the scroll container.

**Rule 4**: Content inside a scroll container must NOT have `flex_1()` or height constraints — it must grow naturally with its children.
