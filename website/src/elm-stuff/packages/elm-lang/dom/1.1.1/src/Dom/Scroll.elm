module Dom.Scroll exposing
  ( toTop, toBottom, y, toY
  , toLeft, toRight, x, toX
  )

{-| When you set `overflow-y: scroll` on an element, a scroll bar will appear
when the content overflows the available space. When that happens, you may want
to modify the scroll position yourself. For example, maybe you have a chat room
that autoscrolls as new messages come in. This module provides functions like
`Dom.Scroll.toBottom` that let you do that kind of thing.

# Vertical
@docs toTop, toBottom, y, toY

# Horizontal
@docs toLeft, toRight, x, toX

-}

import Dom exposing (Error, Id)
import Dom.Size as Size
import Native.Dom
import Task exposing (Task)



-- VERTICAL


{-| Find the node with the given `Id` and scroll it to the top.

So `toTop id` is the same as `toY id 0`.
-}
toTop : Id -> Task Error ()
toTop id =
  toY id 0


{-| Find the node with the given `Id` and scroll it to the bottom.
-}
toBottom : Id -> Task Error ()
toBottom =
  Native.Dom.toBottom


{-| How much this element is scrolled vertically.

Say you have a node that does not fit in its container. A scroll bar shows up.
Initially you are at the top, which means `y` is `0`. If you scroll down 300
pixels, `y` will be `300`.

This is roughly the same as saying [`document.getElementById(id).scrollTop`][docs].

[docs]: https://developer.mozilla.org/en-US/docs/Web/API/Element/scrollTop
-}
y : Id -> Task Error Float
y =
  Native.Dom.getScrollTop


{-| Set the vertical scroll to whatever offset you want.

Imagine you have a chat room and you want to control how it scrolls. Say the
full chat is 400 pixels long, but it is in a box that limits the visible height
to 100 pixels.

  - If we say `toY "chat" 0` it will scroll to the very top.
  - If we say `toY "chat" 300` it will be at the bottom.

If we provide values outside that range, they just get clamped, so
`toY "chat" 900` is also scrolled to the bottom.
-}
toY : Id -> Float -> Task Error ()
toY =
  Native.Dom.setScrollTop



-- HORIZONTAL


{-| Find the node with the given `Id` and scroll it to the far left.

So `toLeft id` is the same as `toX id 0`.
-}
toLeft : Id -> Task Error ()
toLeft id =
  toX id 0


{-| Find the node with the given `Id` and scroll it to the far right.
-}
toRight : Id -> Task Error ()
toRight =
  Native.Dom.toRight


{-| How much this element is scrolled horizontally.

Say you have a node that does not fit in its container. A scroll bar shows up.
Initially you are at the far left, which means `x` is `0`. If you scroll right
300 pixels, `x` will be `300`.

This is roughly the same as saying [`document.getElementById(id).scrollLeft`][docs].

[docs]: https://developer.mozilla.org/en-US/docs/Web/API/Element/scrollLeft
-}
x : Id -> Task Error Float
x =
  Native.Dom.getScrollLeft


{-| Set the horizontal scroll to whatever offset you want.

It works just like `toY`, so check out those docs for a more complete example.
-}
toX : Id -> Float -> Task Error ()
toX =
  Native.Dom.setScrollLeft
