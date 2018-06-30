module Views.Settings exposing (view)

--import Routing exposing (Route(..))
--import Views.Common exposing (..)
--import Views.Header as Header
import Html exposing (..)
--import Html.Attributes exposing (..)
--import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)

import Views.NotFound as NotFound


view : Model -> Html Msg
view model =
    NotFound.view model
--    div [ class "settings" ] [
--        Header.view model,
--        getUser model
--    ]
--
--getUser : Model -> Html Msg
--getUser model =
--    case model.userInfo of
--        Success n ->
--            div [ class "settings" ] [
--                text n.name
--            ]
--        _ ->
--            -- TODO: I want to call without clicking
--            a [ onClick <| AutoLogin ] []
