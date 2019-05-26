const path = require('path');
const ExtractTextPlugin = require('extract-text-webpack-plugin');
const UglifyJsPlugin = require('uglifyjs-webpack-plugin');
const OptimizeCSSAssetsPlugin = require('optimize-css-assets-webpack-plugin');
const CopyWebpackPlugin = require('copy-webpack-plugin');
const AutoPrefixer = require('autoprefixer');


const appCss = new ExtractTextPlugin({
    filename: '../css/app.css'
});
const AutoPrefixerPlugin = AutoPrefixer({
    grid: true,
    browsers: ["last 2 versions", "ie >= 11", "Android >= 4"]
});


module.exports = {
    optimization: {
        minimizer: [
            new UglifyJsPlugin({
                cache: true,
                parallel: true,
                sourceMap: true
            }),
            new OptimizeCSSAssetsPlugin({})
        ]
    },

    entry: {
        index: ['babel-polyfill', './js/app.js']
    }, // TOOD: entryとmodule.exportsから出るのを複数に分ければ，cssをapp.jsでimportする必要がなくなる？

    output: {
        path: path.resolve(__dirname, '../dist/js'),
        filename: 'app.js'
    },

    plugins: [
        appCss,
        new CopyWebpackPlugin([{
            from: 'assets/',
            to: '../'
        }]),
        new CopyWebpackPlugin([{
            from: 'index.html',
            to: '../'
        }])
    ],

    module: {
        rules: [
            {
                test: /\.js$/,
                exclude: /node_modules/,
                use: {
                    loader: 'babel-loader'
                }
            },
            {
                test: /\.scss$/,
                exclude: [/mobile/],
                use: appCss.extract({
                    use: [
                        {
                            loader: "css-loader",
                            options: {
                                url: false,
                                localIdentName: '[local]'
                            }
                        },
                        {
                            loader: "postcss-loader",
                            options: {
                                plugins: [ AutoPrefixerPlugin ]
                            }
                        },
                        "sass-loader"
                    ]
                })
            },
            {
                test: /\.elm$/,
                exclude: [/elm-stuff/, /node_modules/],
                loader: "elm-webpack-loader",
                options: {
                    optimize: true
                }
            },
        ],
        noParse: [/\.elm$/]
    }
};
