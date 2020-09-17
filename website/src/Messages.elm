module Messages exposing (Msg(..))

import Browser exposing (UrlRequest)
import Browser.Dom exposing (Viewport)
import Http
import Model exposing (SearchResponse)
import Time exposing (Posix)
import Url exposing (Url)


type Msg
    = OnUrlRequest UrlRequest
    | OnUrlChange Url
    | OnAnimationFrame Posix
    | OnThemeChange Bool
    | GotNewViewport Viewport
    | GotNewWidth Int
    | OnSearchInput Int String
    | OnEnterPress
    | ReceivePackages (Result Http.Error SearchResponse)
    | ClearPackages
    | NoOp
