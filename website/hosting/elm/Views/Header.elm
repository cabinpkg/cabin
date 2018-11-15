module Views.Header exposing (view)

import Routing exposing (Route(..))
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Views.Svgs as Svgs
import Model exposing (..)
import Html.Styled
import Css
import Css.Global


view : Model -> Html Msg
view model =
    header [ class "header" ] [
        div [ class "header-menu" ]
            [ hambMenu model
            , logo model.width
            ]
    ]


scrollCancel : Html.Styled.Html Msg
scrollCancel =
    Css.Global.global
        [ Css.Global.html
              [ Css.overflow Css.hidden
              , Css.height (Css.pct 100)
              ]
        , Css.Global.body
              [ Css.overflow Css.hidden
              , Css.height (Css.pct 100)
              ]
        ]

scrollCancelBool : Bool -> List (Html.Styled.Html Msg)
scrollCancelBool bool =
    if bool then
        [scrollCancel]
    else
        []

scrollCancelDiv : Bool -> Html.Styled.Html Msg
scrollCancelDiv bool =
    Html.Styled.styled Html.Styled.div
        [] [] (scrollCancelBool bool)


hambMenu : Model -> Html Msg
hambMenu model =
    div [ class "hm_wrap" ]
        [ input [ id "hm_menu"
                , type_ "checkbox"
                , name "hm_menu"
                , class "hm_menu_check"
                , onCheck HandleChecked
                ] []
        , label [ for "hm_menu"
                , class "hm_btn"
                ] []
        , headerMenu model
        , div [ class "hm_menu_close" ]
              [ label [ for "hm_menu" ] [] ]
        , scrollCancelDiv model.isChecked |> Html.Styled.toUnstyled
        ]


logo : Int -> Html Msg
logo widthSize =
    a [ onClick <| NavigateTo HomeIndexRoute
      , class "header-item header-item-logo"
      ] [ Svgs.logo widthSize ]


headerMenu : Model -> Html Msg
headerMenu model =
    let
        appendListItem =
            List.append
            [ menuItemPackages
            , menuItemDonate
            , menuItemDocs
            ]
        listItem =
            appendListItem <| signupOrUserInfo model
        lists =
            List.map toLi listItem
    in
        nav [ class "hm_menu_wrap" ]
        [ ul [ class "header-list-menu" ] lists
        ]


toLi : Html Msg -> Html Msg
toLi item =
    li [] [ item ]



menuItemPackages : Html Msg
menuItemPackages =
    a [ onClick <| NavigateTo (PackagesRoute "")
        , class "header-item"
      ] [ text "PACKAGES" ]


menuItemDonate : Html Msg
menuItemDonate =
    a [ onClick <| NavigateTo DonateRoute
        , class "header-item"
        ] [ text "DONATE" ]


menuItemDocs : Html Msg
menuItemDocs =
    a [ href "https://docs.poac.pm/"
        , class "header-item"
        ] [ text "DOCS" ]


signupOrUserInfo : Model -> List (Html Msg)
signupOrUserInfo model =
    case model.signinUser of
        Success user ->
            [ userInfo user model.signinId ]
        Requesting ->
            [ div [ class "spinner" ]
                  [ Svgs.spinner ]
            ]
        _ ->
            [ signin
            , signup
            ]


userInfo : SigninUser -> String -> Html Msg
userInfo user signinId =
    div [ class "dropdown" ]
    [ button [ class "dropbtn" ]
      [ img [ class "avatar"
            , alt signinId
            , src user.photo_url
            , width 20
            , height 20
            ] []
      , text user.name
      , span [ class "dropdown-caret" ] []
      ]
    , div [ class "dropdown-content" ]
        [ a [ onClick <| NavigateTo (UsersRoute signinId)
            , style [("cursor", "pointer"), ("color", "black")]
            ]
            [ text "Your Profile"
            ]
        , a [ onClick <| NavigateTo SettingRoute ]
            [ text "Settings"
            ]
        , hr [ class "dropdown-divider" ] []
        , a [ onClick <| Signout ]
            [ text "Sign out"
            ]
        ]
    ]

signin : Html Msg
signin =
    a [ class "sign in pulse"
        , onClick <| LoginOrSignup
      ] [ text "SIGNIN" ]

signup : Html Msg
signup =
    a [ class "sign up pulse"
        , onClick <| LoginOrSignup
      ] [ text "SIGNUP" ]
