module Messages exposing (..)

import Model exposing (..)
import Navigation
import Routing exposing (Route)
import Http
import Scroll exposing (Move)


type Msg
    = UrlChange Navigation.Location
    | NavigateTo Route
    | LoginOrSignup
    | Signin User
    | Signout
    | HandleSearchInput String
    | HandleTokenInput String
    | FetchUser (Maybe User)
    | FetchToken (List Token)
    | CreateToken
    | RevokeToken String
    | FetchPackages (List Package)
    | FetchDetailedPackage (Maybe DetailedPackage)
    | ScrollHandle Move
    | Fadein FadeinType
    | OnSearchInput String
    | Search Int

type FadeinType
    = GetStart
    | Section1
