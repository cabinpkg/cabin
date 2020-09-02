module Page exposing (view)

import Browser
import Html exposing (..)
import Messages exposing (Msg)
import Model exposing (Model)
import Route exposing (Route)
import String.Extra exposing (humanize)
import Page.Footer as Footer
import Page.Header as Header
import Page.Home as Home
import Page.NotFound as NotFound
import Page.Packages as Packages
import Page.Policies as Policies



view : Model -> Browser.Document Msg
view model =
    let
        ( title, html ) =
            currentPage model
    in
    { title = title
    , body =
          [ Header.view model
          , html
          , Footer.view
          ]
    }


currentPage : Model -> ( String, Html Msg )
currentPage model =
    case model.route of
        Route.Home ->
            ( "Poac Package Manager for C++", Home.view model )

        Route.Packages ->
            ( "Poac Packages", Packages.view model )

        Route.Policy ->
            ( "Policies", Policies.view "" )

        Route.Policies name ->
            ( humanize name , Policies.view name )

        Route.NotFound ->
            ( "Not Found", NotFound.view )
