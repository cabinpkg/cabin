module View exposing (view)

import Views.Header as Header
import Views.Footer as Footer
import Views.Index as Index
import Views.Packages as Packages
import Views.Donate as Donate
import Views.Users as Users
import Views.Settings as Settings
import Views.NotFound as NotFound
import Model exposing (Model)
import Html exposing (..)
import Messages exposing (Msg)
import Routing exposing (Route(..))


attach : Model -> Html Msg -> Html Msg
attach model html =
    main_ []
      [ Header.view model
      , html
      , Footer.view ]

view : Model -> Html Msg
view model =
    let
        attachModel =
            attach model
    in
        attachModel
        <| case model.route of
            HomeIndexRoute ->
                Index.view model

            PackagesRoute name ->
                Packages.view model name

            OrgPackagesRoute org name ->
                Packages.view model (org ++ "/" ++ name)

            DonateRoute ->
                Donate.view model

            UsersRoute id ->
                Users.view model id

            SettingsRoute id ->
                Settings.view model id

            SettingRoute ->
                Settings.view model "profile"

            NotFoundRoute ->
                NotFound.view
