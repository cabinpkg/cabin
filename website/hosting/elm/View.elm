module View exposing (view)

import Browser
import Html exposing (..)
import Messages exposing (Msg)
import Model exposing (Model)
import Route exposing (Route)
import String.Extra exposing (humanize)
import Views.Footer as Footer
import Views.Footers.Policies as Policies
import Views.Header as Header
import Views.Home as Home
import Views.NotFound as NotFound
import Views.OwnPackages as OwnPackages
import Views.Package as Package
import Views.PackageList as PackageList
import Views.PackageVersions as PackageVersions



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

        Route.PackageList ->
            ( "Poac Packages", PackageList.view model )

        Route.OwnPackages owner ->
            ( owner, OwnPackages.view model owner )

        Route.PackageVersions owner repo ->
            ( owner ++ "/" ++ repo
            , PackageVersions.view model owner repo
            )

        Route.Package owner repo version ->
            ( owner ++ "/" ++ repo ++ " " ++ version
            , Package.view model
            )

        Route.Policy ->
            ( "Policies", Policies.view "" )

        Route.Policies name ->
            ( humanize name , Policies.view name )

        Route.NotFound ->
            ( "Not Found", NotFound.view )
