module Models exposing (..)

import Players.Models exposing (Player)
import Routing


type alias Model =
    { players : List Player
    , route : Routing.Route
    }

initialModel : Routing.Route -> Model
initialModel route =
    { players = []
    , route = route
    }