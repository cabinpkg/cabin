module Model exposing (..)

import Browser.Navigation exposing (Key)
import Route exposing (Route)


type alias Flags =
    { api : String
    }


type alias IsFadein =
    { section1 : Bool
    , getStart : Bool
    }


type alias Model =
    { flags : Flags
    , navKey : Key
    , route : Route
    , width : Int
    , isFadein : IsFadein
    , searchInput : String
    , isChecked : Bool
    }
