module Messages exposing (Msg(..))

import Browser exposing (UrlRequest)
import Browser.Dom exposing (Viewport)
import Url exposing (Url)
import Time exposing (Posix)


type Msg
    = OnUrlChange Url
    | OnUrlRequest UrlRequest
    | OnAnimationFrame Posix
    | GotNewViewport Viewport
    | GotNewWidth Int
    | OnSearchInput String
    | Search Int
