module Model exposing (..)

import Routing exposing (Route)
import Http exposing (Request)


type RemoteData e a
    = NotRequested
    | Requesting -- TODO: Make effective use
    | Failure e
    | Success a


type alias Token =
    { id : String
    , name : String
    , owner : String
    , created_date : String
    , last_used_date : Maybe String
    }

type alias User =
    { id : String
    , name : String
    , photo_url : String
    , github_link : String
    }


type alias Model =
    { route : Route
    , loginUser : RemoteData String User
    , otherUser : RemoteData String User
    , currentToken : RemoteData String (List Token)
    , search : String
    , newTokenName : String
    }


initialModel : Route -> Model
initialModel route =
    { route = route
    , loginUser = NotRequested
    , otherUser = NotRequested
    , currentToken = NotRequested
    , search = ""
    , newTokenName = ""
    }
