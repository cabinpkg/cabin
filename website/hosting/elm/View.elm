module View exposing (view)

import Browser
import Html exposing (..)
import Messages exposing (Msg)
import Model exposing (Model)
import Routing exposing (Route(..))
import String.Extra exposing (humanize)
import Views.Donate as Donate
import Views.Footer as Footer
import Views.Header as Header
import Views.Index as Index
import Views.NotFound as NotFound
import Views.Packages as Packages
import Views.Settings as Settings
import Views.Users as Users


view : Model -> Browser.Document Msg
view model =
    let
        ( title, html ) = currentPage model
    in
    { title = title
    , body = [ html ]
    }


attach : Model -> ( String, Html Msg ) -> ( String, Html Msg )
attach model ( title, html ) =
    ( title
    , main_ []
        [ Header.view model
        , html
        , Footer.view
        ]
    )


currentPage : Model -> ( String, Html Msg )
currentPage model =
    attach model <|
    case model.route of
        HomeIndexRoute ->
            ( "poac - Package Manager for C++", Index.view model )

        PackagesRoute name ->
            ( "poac - " ++ name, Packages.view model name )

        OrgPackagesRoute org name ->
            let
                org_and_name = org ++ "/" ++ name
            in
            ( "poac - " ++ org_and_name, Packages.view model org_and_name )

        DonateRoute ->
            ( "poac - Donate", Donate.view model )

        UsersRoute id ->
            ( "poac - " ++ id, Users.view model id )

        SettingsRoute id ->
            ( "poac - " ++ humanize id , Settings.view model id )

        SettingRoute ->
            ( "poac - Tokens", Settings.view model "tokens" )

        NotFoundRoute ->
            ( "poac - Page not found", NotFound.view )
