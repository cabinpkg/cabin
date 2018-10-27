module Dom exposing
  ( focus, blur, Id
  , Error(..)
  )

{-|

# Focus
@docs focus, blur, Id

# Errors
@docs Error

-}

import Native.Dom
import Task exposing (Task)



-- ERRORS


{-| All the functions here look up DOM nodes by ID. If you ask for an ID that
is not currently attached to the DOM, you will get this error!
-}
type Error = NotFound String



-- FOCUS


{-| A unique identifier for a particular DOM node. When you create
`<div id="my-thing"></div>` you would refer to it with the `Id` `"my-thing"`.
-}
type alias Id =
  String


{-| On a website, there can only be one thing in focus at a time. A text field,
a check box, etc. This function tells the Elm runtime to move the focus to a
particular DOM node.

    Dom.focus "my-thing"

This is roughly the same as saying `document.getElementById(id).focus()`.

NOTE: setting focus can silently fail if the element is invisible. This could be captured as an error by checking to see
if document.activeElement actually got updated to the element we selected. https://jsbin.com/xeletez/edit?html,js,output
-}
focus : Id -> Task Error ()
focus =
  Native.Dom.focus


{-| On a website, there can only be one thing in focus at a time. A text field,
a check box, etc. Sometimes you want that thing to no longer be in focus. This
is called &ldquo;blur&rdquo; for reasons that are unclear to almost everybody.
So this function tells a particular DOM node to lose focus.

    Dom.blur "my-thing"

This is roughly the same as saying `document.getElementById(id).blur()`.
-}
blur : Id -> Task Error ()
blur =
  Native.Dom.blur

