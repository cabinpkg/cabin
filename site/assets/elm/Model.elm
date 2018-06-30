module Model exposing (..)

import Routing exposing (Route)
import Http exposing (Request)


type RemoteData e a
    = NotRequested
    | Requesting
    | Failure e
    | Success a


type alias User =
    { id : String
    , name : String
    , avatar : String
    , apikey : Maybe String
    , github : String
    , published_packages : Maybe (List String)
    }

type alias Model =
    { route : Route
    , loginUser : RemoteData String User
    , otherUser : RemoteData String User
    , search : String
    , csrfToken : String
    }


initialModel : Route -> Model
initialModel route =
    { route = route
    , loginUser = NotRequested
    , otherUser = NotRequested
    , search = ""
    , csrfToken = ""
    }
