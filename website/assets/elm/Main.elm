module Main exposing (..)

import Messages exposing (Msg(UrlChange))
import Model exposing (Model, initialModel)
import Navigation
import Routing exposing (parse)
import Update exposing (update, urlUpdate)
import View exposing (view)
import Subscriptions exposing (subscriptions)


init : Int -> Navigation.Location -> ( Model, Cmd Msg )
init seed location =
    let
        currentRoute =
            parse location

        model =
            initialModel seed currentRoute
    in
        urlUpdate model


main : Program Int Model Msg
main =
    Navigation.programWithFlags UrlChange
        { init = init
        , view = view
        , update = update
        , subscriptions = subscriptions
        }
