module Main exposing (..)

import Messages exposing (Msg(..))
import Model exposing (..)
import Navigation
import Routing exposing (parse)
import Update exposing (..)
import View exposing (view)


init : Navigation.Location -> ( Model, Cmd Msg )
init location =
    let
        currentRoute =
            parse location

        model =
            initialModel currentRoute
    in
        urlUpdate model


main : Program Never Model Msg
main =
    Navigation.program UrlChange
        { init = init
        , view = view
        , update = update
        , subscriptions = always <| Sub.none
        }
