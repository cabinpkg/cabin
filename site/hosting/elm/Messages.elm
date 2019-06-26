module Messages exposing (FadeinType(..), Msg(..))

import Browser exposing (UrlRequest)
import Model exposing (..)
import Url exposing (Url)
import Http


type Msg
    = OnUrlChange Url
    | OnUrlRequest UrlRequest
    | HandleSearchInput String
    | FetchPackages (List Package)
    | FetchDetailedPackage (Maybe DetailedPackage)
    | ScrollHandle Int
    | OnWidthHandle Int
    | Fadein FadeinType
    | OnSearchInput String
    | Search Int
    | HandleChecked Bool
    | FetchReadme (Result Http.Error String)


type FadeinType
    = GetStart
    | Section1
