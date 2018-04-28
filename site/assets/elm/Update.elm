module Update exposing (..)

import Commands exposing (fetch, fetchContact)
import Messages exposing (..)
import Model exposing (..)
import Navigation
import Routing exposing (Route(..), parse, toPath)


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        FetchResult (Ok response) ->
            { model | contactList = Success response } ! []

        FetchResult (Err error) ->
            { model | contactList = Failure "Something went wrong..." } ! []

        Paginate pageNumber ->
            model ! [ fetch pageNumber model.search ]

        HandleSearchInput value ->
            { model | search = value } ! []

        HandleFormSubmit ->
            { model | contactList = Requesting } ! [ fetch 1 model.search ]

        ResetSearch ->
            { model | search = "" } ! [ fetch 1 "" ]

        UrlChange location ->
            let
                currentRoute =
                    parse location
            in
                urlUpdate { model | route = currentRoute }

        NavigateTo route ->
            model ! [ Navigation.newUrl <| toPath route ]

        FetchContactResult (Ok response) ->
            { model | contact = Success response } ! []

        FetchContactResult (Err error) ->
            { model | contact = Failure "Contact not found" } ! []


urlUpdate : Model -> ( Model, Cmd Msg )
urlUpdate model =
    case model.route of
        HomeIndexRoute ->
            case model.contactList of
                NotRequested ->
                    model ! [ fetch 1 "" ]

                _ ->
                    model ! []

        ShowContactRoute id ->
            { model | contact = Requesting } ! [ fetchContact id ]

        _ ->
            model ! []
