module Messages exposing (FadeinType(..), Msg(..))

import Browser exposing (UrlRequest)
import Model exposing (..)
import Url exposing (Url)
import Http


type Msg
    = OnUrlChange Url
    | OnUrlRequest UrlRequest
    | ScrollHandle Int
    | Fadein FadeinType
    | OnWidthHandle Int
    | OnSearchInput String
    | Search Int
    | HandleChecked Bool
    | FetchOwnPackages (Maybe (List String))
    | FetchPackageVersions (Maybe (List String))
    | FetchPackage (Maybe String)
    | FetchReadme (Result Http.Error String)


type FadeinType
    = GetStart
    | Section1
