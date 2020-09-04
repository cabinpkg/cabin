module Model exposing (..)

import Browser.Navigation exposing (Key)
import Route exposing (Route)


type alias Flags =
    { algoliaApiKey : String
    , algoliaApplicationId : String
    }


type alias IsFadein =
    { abstract : Bool
    , section1 : Bool
    , demo : Bool
    , getStart : Bool
    }


type alias Model =
    { flags : Flags
    , navKey : Key
    , route : Route
    , isFadein : IsFadein
    , searchInput : String
    , isChecked : Bool
    }
