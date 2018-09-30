module View exposing (view)

import Views.Index as Index
import Views.Packages as Packages
import Views.Donate as Donate
import Views.Users as Users
import Views.Settings as Settings
import Views.NotFound as NotFound
import Model exposing (Model)
import Html exposing (Html)
--import Html.Lazy exposing (lazy, lazy2)
import Messages exposing (Msg)
import Routing exposing (Route(..))


view : Model -> Html Msg
view model =
    case model.route of
        HomeIndexRoute ->
            Index.view model

        PackagesRoute name ->
            Packages.view model name

        DonateRoute ->
            Donate.view model

        UsersRoute id ->
            Users.view model id

        SettingsRoute id ->
            Settings.view model id

        SettingRoute ->
            Settings.view model "profile"

        NotFoundRoute ->
            NotFound.view model
