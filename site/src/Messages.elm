module Messages exposing (Msg(..))

import Browser exposing (UrlRequest)
import Browser.Dom exposing (Viewport)
import Url exposing (Url)
import Time exposing (Posix)
import Model exposing (IsFadein)



type Msg
    = OnUrlChange Url
    | OnUrlRequest UrlRequest
    | OnAnimationFrame Posix
    | GotNewViewport Viewport
    | Fadein FadeinType
    | GotNewWidth Int
    | OnSearchInput String
    | Search Int
    | HandleChecked Bool


type alias FadeinType =
    IsFadein -> Bool -> IsFadein
