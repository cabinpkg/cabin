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
    | Signin (Maybe SigninUser)
    | Signout
    | FetchSigninId (Maybe String)
    | HandleSearchInput String
    | HandleTokenInput String
    | FetchUser (Maybe User)
    | FetchToken (List Token)
    | CreateToken
    | RevokeToken String
    | FetchPackages (List Package)
    | FetchDetailedPackage (Maybe DetailedPackage)
    | ScrollHandle Move
    | OnWidthHandle Int
    | Fadein FadeinType
    | OnSearchInput String
    | Search Int
    | HandleChecked Bool

type FadeinType
    = GetStart
    | Section1
