module Page exposing (view, toUnstyledDocument)

import Browser
import Html
import Html.ResetCss exposing (ericMeyer)
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
import Html.Styled exposing (Html, toUnstyled)



type alias Document msg =
    { title : String
    , body : List (Html msg)
    }


toUnstyledDocument : Document msg -> Browser.Document msg
toUnstyledDocument doc =
    { title = doc.title
    , body = List.map toUnstyled doc.body
    }


view : Model -> Document Msg
view model =
    --let
    --    ( title, html ) =
    --        currentPage model
    --in
    { title = "hoge" -- title
    , body =
          [ Footer.view model ]
          --[ ericMeyer
          --, Header.view model
          --, html
          --, Footer.view
          --]
    }


--currentPage : Model -> ( String, Html Msg )
--currentPage model =
--    case model.route of
--        Route.Home ->
--            ( "Poac Package Manager for C++", Home.view model )
--
--        Route.Packages ->
--            ( "Poac Packages", Packages.view model )
--
--        Route.Policy ->
--            ( "Policies", Policies.view "" )
--
--        Route.Policies name ->
--            ( humanize name , Policies.view name )
--
--        Route.NotFound ->
--            ( "Not Found", NotFound.view )
