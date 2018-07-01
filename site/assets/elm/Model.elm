module Model exposing (..)

import Routing exposing (Route)
import Http exposing (Request)
import Uuid exposing (Uuid)
import Random.Pcg exposing (Seed, initialSeed)


type RemoteData e a
    = NotRequested
    | Requesting
    | Failure e
    | Success a


type alias User =
    { id : String
    , name : String
    , avatar : String
    , apikeyName : Maybe String
    , github : String
    , published_packages : Maybe (List String)
    }

type alias Model =
    { route : Route
    , loginUser : RemoteData String User
    , otherUser : RemoteData String User
    , search : String
    , csrfToken : String
    , currentSeed : Seed
    , currentUuid : Maybe (List Uuid)
    }


initialModel : Int -> Route -> Model
initialModel seed route =
    { route = route
    , loginUser = NotRequested
    , otherUser = NotRequested
    , search = ""
    , csrfToken = ""
    , currentSeed = initialSeed seed
    , currentUuid = Nothing
    }
