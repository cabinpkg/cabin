module Model exposing (..)

import Routing exposing (Route)
import Http exposing (Request)
import Uuid exposing (Uuid)
import Random.Pcg exposing (Seed, initialSeed)


type RemoteData e a
    = NotRequested
    | Requesting -- TODO: Make effective use
    | Failure e
    | Success a


type alias Token =
    { id : String
    , name : String
    , created_date : String
    , last_used_date : Maybe String
    }

type alias User =
    { id : String
    , name : String
    , token : Maybe (List String)
    , avatar_url : String
    , github_link : String
    , published_packages : Maybe (List String)
    }

type alias Model =
    { route : Route
    , loginUser : RemoteData String User
    , otherUser : RemoteData String User
    , search : String
    , csrfToken : String
    , currentSeed : Seed
    , currentUuidList : Maybe (List String)
    }


initialModel : Int -> Route -> Model
initialModel seed route =
    { route = route
    , loginUser = NotRequested
    , otherUser = NotRequested
    , search = ""
    , csrfToken = ""
    , currentSeed = initialSeed seed
    , currentUuidList = Nothing
    }
