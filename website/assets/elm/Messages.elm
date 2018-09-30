module Messages exposing (..)

--import Http
import Model exposing (..)
import Navigation
import Routing exposing (Route)


type Msg
    = UrlChange Navigation.Location
    | NavigateTo Route
    | LoginOrSignup
    | Login User
    | Logout
    | HandleSearchInput String
    | HandleTokenInput String
    | FetchUser (Maybe User)
    | FetchToken (List Token)
    | CreateToken
    | RevokeToken String
    | FetchPackages (List Package)
    | FetchDetailedPackage (Maybe DetailedPackage)
