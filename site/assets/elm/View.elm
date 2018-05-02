module View exposing (..)

import Views.Contact exposing (showContactView)
import Views.Index exposing (indexView)
import Views.Packages exposing (packagesView)
import Views.Donation exposing (donationView)
import Views.NotFound exposing (notFoundView)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (onClick)
import Messages exposing (..)
import Model exposing (..)
import Routing exposing (Route(..))


view : Model -> Html Msg
view model =
    case model.route of
        HomeIndexRoute ->
            indexView model

        PackagesRoute ->
            packagesView model

        DonationRoute ->
            donationView model

        NotFoundRoute ->
            notFoundView
