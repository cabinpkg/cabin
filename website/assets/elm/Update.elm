module Update exposing (..)

import Commands exposing (..)
import Messages exposing (..)
import Model exposing (..)
import Navigation
import Routing exposing (Route(..), parse, toPath)


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        UrlChange location ->
            let
                currentRoute =
                    parse location
            in
                urlUpdate { model | route = currentRoute }

        NavigateTo route ->
            model ! [ Navigation.newUrl <| toPath route ]

        HandleSearchInput value ->
            { model | search = value } ! []

        UserResult (Ok response) ->
            { model | userInfo = Success response } ! []
        UserResult (Err error) ->
            { model | userInfo = Failure (toString error) } ! []

        CsrfTokenResponse (Ok response) ->
            model ! [ logout response ]
        CsrfTokenResponse (Err error) ->
            model ! [ Debug.crash (toString error) ]

        DeleteSession ->
            model ! [ updateToken ]

        PostDeleted (Ok response) ->
            { model | userInfo = NotRequested } ! []
        PostDeleted (Err error) ->
            model ! [ Debug.crash (toString error) ]

--        _ ->
--            model ! []

urlUpdate : Model -> ( Model, Cmd Msg )
urlUpdate model =
    case model.route of
        HomeIndexRoute ->
            case model.userInfo of
                NotRequested ->
                    model ! [ getSession ]
                _ ->
                    model ! []
        _ ->
            model ! []
