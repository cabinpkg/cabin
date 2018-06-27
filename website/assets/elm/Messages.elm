module Messages exposing (..)

import Http
import Model exposing (..)
import Navigation
import Routing exposing (Route)


type Msg
    = UrlChange Navigation.Location
    | NavigateTo Route
    | AutoLogin
    | KeyDown Int
    | FocusOn
    | SelectMeta String
    | HandleSearchInput String
    | UserResult (Result Http.Error UserInfo)
    | PostDeleted (Result Http.Error String)
    | DeleteSession
