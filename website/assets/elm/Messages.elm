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
    | LoginUserResult (Result Http.Error User)
    | OtherUserResult (Result Http.Error User)
    | TokenUpdated (Result Http.Error User)
    | PostDeleted (Result Http.Error String)
    | DeleteSession
    | NewUuid
