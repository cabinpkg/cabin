module View exposing (view)

import Browser
import Html exposing (..)
import Html.Lazy exposing (..)
import Messages exposing (Msg)
import Model exposing (Model)
import Routing exposing (Route(..))
import String.Extra exposing (humanize)
import Views.Footer as Footer
import Views.Footers.Policies as Policies
import Views.Header as Header
import Views.Index as Index
import Views.NotFound as NotFound
import Views.Packages as Packages


view : Model -> Browser.Document Msg
view model =
    let
        title_prefix = "Poac"
        ( title, html ) = currentPage model
    in
    { title = title_prefix ++ title
    , body = [ Header.view model
             , html
             , Footer.view
             ]
    }


currentPage : Model -> ( String, Html Msg )
currentPage model =
    case model.route of
        HomeIndexRoute ->
            ( " Package Manager for C++", Index.view model )

        PackagesRoute ->
            ( " - Packages", Packages.view model "" )

        PackageRoute name ->
            ( " - " ++ name, Packages.view model name )

        OrgPackageRoute org name ->
            let
                org_and_name = org ++ "/" ++ name
            in
            ( " - " ++ org_and_name, Packages.view model org_and_name )

        PolicyRoute ->
            ( " - Policies", lazy Policies.view "" )

        PoliciesRoute name ->
            ( " - " ++ humanize name , lazy Policies.view name )

        NotFoundRoute ->
            ( " - Page not found", NotFound.view )
