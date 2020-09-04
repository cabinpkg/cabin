module Messages exposing (FadeinType(..), Msg(..))

import Browser exposing (UrlRequest)
import Browser.Dom exposing (Viewport)
import Url exposing (Url)
import Time exposing (Posix)


type Msg
    = OnUrlChange Url
    | OnUrlRequest UrlRequest
    | GetNewViewport Posix
    | ScrollHandle Viewport
    | Fadein FadeinType
    | OnWidthHandle Int
    | OnSearchInput String
    | Search Int
    | HandleChecked Bool


type FadeinType
    = GetStart
    | Section1
