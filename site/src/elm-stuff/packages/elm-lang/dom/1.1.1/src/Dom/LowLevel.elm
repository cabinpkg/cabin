module Dom.LowLevel exposing
  ( onDocument
  , onWindow
  )

{-| This is not for general use. It backs libraries like `elm-lang/mouse` and
`elm-lang/window` which should cover your needs in most cases. In the rare
case that those packages do not seem to cover your scenario, first bring it up
with the community. Ask around and learn stuff first! Only get into these
functions after that.

# Global Event Listeners
@docs onDocument, onWindow

-}

import Json.Decode as Json
import Native.Dom
import Task exposing (Task)


{-| Add an event handler on the `document`. The resulting task will never end,
and when you kill the process it is on, it will detach the relevant JavaScript
event listener.
-}
onDocument : String -> Json.Decoder msg -> (msg -> Task Never ()) -> Task Never Never
onDocument =
  Native.Dom.onDocument


{-| Add an event handler on `window`. The resulting task will never end, and
when you kill the process it is on, it will detach the relevant JavaScript
event listener.
-}
onWindow : String -> Json.Decoder msg -> (msg -> Task Never ()) -> Task Never Never
onWindow =
  Native.Dom.onWindow
