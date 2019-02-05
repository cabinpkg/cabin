module Messages exposing (FadeinType(..), Msg(..))

import Browser exposing (UrlRequest)
import Model exposing (..)
import Routing exposing (Route)
import Url exposing (Url)
--import Scroll exposing (Move)


type Msg
    = OnUrlChange Url
    | OnUrlRequest UrlRequest
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
    | DeletePackage String String
    | FetchPackages (List Package)
    | FetchDetailedPackage (Maybe DetailedPackage)
--    | ScrollHandle Move
    | OnWidthHandle Int
    | Fadein FadeinType
    | OnSearchInput String
    | Search Int
    | HandleChecked Bool


type FadeinType
    = GetStart
    | Section1
