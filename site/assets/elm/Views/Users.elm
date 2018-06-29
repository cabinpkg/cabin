module Views.Users exposing (view)

import Routing exposing (Route(..))
import Views.Common exposing (..)
import Views.Header as Header
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)


view : Model -> String -> Html Msg
view model userId =
    div [ class "users" ] [
        Header.view model,
        user model
    ]

user : Model -> Html Msg
user model =
    case model.userInfo of
        Success n ->
            div [ class "info" ] [
                img [ class "avatar top", alt n.usrId, src n.imgUrl, width 200, height 200 ] [],
                h2 [ class "user-id" ] [ text n.usrId ]
            ]
        _ ->
            a [ href "/auth" ] [ text "Login with GitHub" ]
