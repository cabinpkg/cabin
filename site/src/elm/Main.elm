module Main exposing (..)

import Messages exposing (Msg(UrlChange))
import Model exposing (Model, initialModel)
import Navigation
import Routing exposing (parse)
import Update exposing (update, urlUpdate)
import View exposing (view)
import Subscriptions exposing (subscriptions)


init : Navigation.Location -> ( Model, Cmd Msg )
init url =
    let
        currentRoute =
            parse url

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
        , subscriptions = subscriptions
        }
